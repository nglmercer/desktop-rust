/**
 * Typed frontend client for the Rust application service — the single way
 * the React renderer talks to the backend. Owns transports (Wry via
 * `window.__desktop` and WebSocket via `desktop-rs serve`), error/cancel
 * semantics, subscriptions and reconnection.
 *
 * Transport selection is automatic:
 * - When `window.__desktop.invoke` exists (Tao/Wry shell with injected
 *   `web/ipc-shim.js`), use Wry IPC. The shim already implements
 *   correlated `{__rpc,id,channel,args}` / `%RpcResult` and
 *   profile-dependent `PENDING_LIMIT`.
 * - Otherwise, when the page was served by the loopback bridge (Termux /
 *   `desktop-rs serve`, URL contains `#token=`), connect a WebSocket to
 *   `/ws`, authenticate with the session token and speak the bridge JSON
 *   protocol (`{id,method,request}` / `{id,result|error,error_code}`).
 *
 * Both transports share the contract in `contracts.ts`, `IBackendError`
 * propagation and cooperative cancellation. The renderer never builds raw
 * Git arguments nor calls a generic arbitrary executor (`desktop.io.git-
 * exec`/`desktop.io.http-fetch`) to implement workflows — those channels
 * are for low-level shims (`fs.ts`, `dugite.ts`, `keytar.ts`) behind the
 * shims, not for application workflows. Application operations go through
 * `request()` / typed helpers below.
 */

import { Capabilities, IBackendError, ReadyPayload } from './contracts'

export interface BackendRequestOptions {
  readonly repositoryId?: string
  readonly accountId?: string
  readonly deadlineMs?: number
  readonly signal?: AbortSignal
}

export class BackendError extends Error implements IBackendError {
  readonly code: string
  readonly retryable: boolean
  readonly jobId?: string
  readonly recoveryId?: string
  readonly details?: Readonly<Record<string, unknown>>

  constructor(error: IBackendError) {
    super(error.message)
    this.code = error.code
    this.retryable = error.retryable
    this.jobId = error.jobId
    this.recoveryId = error.recoveryId
    this.details = error.details
    this.name = 'BackendError'
  }
}

function toBackendError(value: unknown): BackendError {
  if (value instanceof BackendError) return value
  const anyValue = value as Partial<IBackendError> & { code?: unknown; message?: unknown; error_code?: unknown; error?: unknown }
  const message =
    typeof anyValue?.message === 'string'
      ? anyValue.message
      : typeof anyValue?.error === 'string'
        ? anyValue.error
        : String(value ?? 'unknown backend error')
  const code =
    typeof anyValue?.code === 'string'
      ? anyValue.code
      : typeof anyValue?.error_code === 'string'
        ? anyValue.error_code
        : 'internal'
  return new BackendError({
    code,
    message,
    retryable: code === 'cancelled' || code === 'queue_full' || (anyValue?.retryable === true),
    jobId: typeof anyValue?.jobId === 'string' ? anyValue.jobId : undefined,
    recoveryId: typeof anyValue?.recoveryId === 'string' ? anyValue.recoveryId : undefined,
    details: anyValue?.details as IBackendError['details'],
  })
}

/** Profile-derived pending limit. The Rust limits are the authority; the
 *  shim exposes `__health` with `pending_promises`. We default to the
 *  normal profile (128) and honour `low-memory` (64) when presented. */
function pendingLimitFromHealth(): number {
  const desktop = (window as any)?.__desktop as { pendingPromises?: number } | undefined
  if (typeof desktop?.pendingPromises === 'number') return desktop.pendingPromises
  const profile =
    typeof (window as any)?.__desktopEnv?.DESKTOP_RESOURCE_PROFILE === 'string'
      ? (window as any).__desktopEnv.DESKTOP_RESOURCE_PROFILE
      : typeof (window as any)?.DESKTOP_RESOURCE_PROFILE === 'string'
        ? (window as any).DESKTOP_RESOURCE_PROFILE
        : undefined
  return profile === 'low-memory' ? 64 : 128
}

type WryBridge = {
  invoke: (channel: string, ...args: unknown[]) => Promise<unknown>
  send: (channel: string, ...args: unknown[]) => void
  on: (channel: string, listener: (event: unknown, ...args: unknown[]) => void) => unknown
  off?: (channel: string, listener?: unknown) => void
}

// ---------------------------------------------------------------------------
// Wry transport
// ---------------------------------------------------------------------------

class WryTransport {
  constructor(private readonly bridge: WryBridge) {}

  get ready(): boolean {
    return typeof this.bridge?.invoke === 'function'
  }

  capabilities(): Capabilities {
    // Wry does not negotiate a version on connect; the Rust shell reports
    // the same capabilities as the bridge (cancel + profile max_pending).
    return {
      cancel: true,
      max_pending: pendingLimitFromHealth(),
    }
  }

  async request(method: string, params: unknown, opts: BackendRequestOptions): Promise<unknown> {
    if (opts.signal?.aborted) {
      throw new BackendError({ code: 'cancelled', message: 'Operation cancelled before start', retryable: true })
    }
    const channel = `desktop.backend.${method}`
    try {
      const value = await this.bridge.invoke(channel, params)
      return value
    } catch (error) {
      throw toBackendError(error)
    }
  }

  cancel(_requestId: string): void {
    // Wry IPC cancellation is cooperative via the `cancel` channel; each
    // in-flight RPC carries a cancellation id known to the Rust executor.
    // The shim's `invoke` itself does not expose a per-request abort; we
    // forward to the backend cancel lane when the shell supports it.
    try {
      this.bridge.send('desktop.io.cancel', {})
    } catch {
      // No cancel lane — cancellation is signal-driven on the Rust side.
    }
  }
}

// ---------------------------------------------------------------------------
// WebSocket transport (Termux / desktop-rs serve)
// ---------------------------------------------------------------------------

interface BridgeRpcReply {
  id?: number
  result?: unknown
  error?: string
  error_code?: string
  event?: string
  snapshot?: unknown
  preferences?: unknown
}

class WebSocketTransport implements AsyncIterableIterator<unknown> {
  private socket: WebSocket | null = null
  private wsReady: Promise<ReadyPayload> | null = null
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void; timer: ReturnType<typeof setTimeout> }>()
  private listeners = new Map<string, Set<(...args: unknown[]) => void>>()
  private nextId = 1
  private token: string | null = null
  private _ready: ReadyPayload | null = null
  private queue: unknown[] = []
  private waiters: Array<(v: unknown) => void> = []
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closed = false
  private readyWaiters: Array<{ resolve: (v: ReadyPayload) => void; reject: (e: unknown) => void }> = []

  constructor() {
    this.token = new URLSearchParams(window.location.hash.slice(1)).get('token') || sessionStorage.getItem('desktopBridgeToken')
    if (this.token) sessionStorage.setItem('desktopBridgeToken', this.token)
  }

  get ready(): boolean {
    return this._ready !== null
  }

  get readyPayload(): ReadyPayload | null {
    return this._ready
  }

  capabilities(): Capabilities {
    return this._ready?.capabilities ?? { cancel: true, max_pending: 32 }
  }

  connect(): Promise<ReadyPayload> {
    if (this.wsReady) return this.wsReady
    this.wsReady = new Promise<ReadyPayload>((resolve, reject) => {
      this.readyWaiters.push({ resolve, reject })
    })
    this.open()
    return this.wsReady
  }

  private open(): void {
    if (!this.token) {
      this.failReady(new Error('missing bridge token in URL fragment'))
      return
    }
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const url = `${scheme}//${window.location.host}/ws`
    const socket = new WebSocket(url)
    this.socket = socket
    socket.onmessage = event => {
      try {
        const data = JSON.parse(event.data) as BridgeRpcReply & { ready?: boolean; version?: number; capabilities?: Capabilities & { cancel: boolean; max_pending: number } }
        if (data.ready) {
          this._ready = {
            ready: true,
            version: (data.version as 3 | undefined) ?? 3,
            capabilities: { cancel: data.capabilities?.cancel ?? true, max_pending: data.capabilities?.max_pending ?? 32 },
          }
          this.flushReady()
          return
        }
        if (typeof data.id === 'number') {
          const entry = this.pending.get(data.id)
          if (entry) {
            this.pending.delete(data.id)
            clearTimeout(entry.timer)
            if (data.error !== undefined) {
              entry.reject(
                new BackendError({
                  code: data.error_code ?? 'internal',
                  message: data.error,
                  retryable: data.error_code === 'cancelled' || data.error_code === 'queue_full',
                })
              )
            } else {
              entry.resolve(data.result)
            }
          }
          return
        }
        if (data.event) {
          this.dispatch(data.event, data.snapshot ?? data.preferences ?? data)
        }
      } catch (error) {
        // Non-JSON frame — ignore; bridge only sends JSON text.
      }
    }
    socket.onopen = () => {
      socket.send(JSON.stringify({ token: this.token }))
    }
    socket.onclose = () => {
      this.socket = null
      this._ready = null
      this.rejectPending(new BackendError({ code: 'disconnected', message: 'bridge socket closed', retryable: true }))
      if (!this.closed) this.scheduleReconnect()
    }
    socket.onerror = () => {
      // onclose follows; no extra handling.
    }
  }

  private flushReady(): void {
    if (!this._ready) return
    for (const w of this.readyWaiters.splice(0)) w.resolve(this._ready)
    this.wsReady = Promise.resolve(this._ready)
  }

  private failReady(error: unknown): void {
    for (const w of this.readyWaiters.splice(0)) w.reject(error)
  }

  private rejectPending(error: unknown): void {
    for (const entry of this.pending.values()) {
      clearTimeout(entry.timer)
      entry.reject(error)
    }
    this.pending.clear()
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer || this.closed) return
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.wsReady = null
      this.open()
    }, 1000)
  }

  async request(method: string, params: unknown, opts: BackendRequestOptions): Promise<unknown> {
    if (opts.signal?.aborted) {
      throw new BackendError({ code: 'cancelled', message: 'Operation cancelled before start', retryable: true })
    }
    if (!this._ready) await this.connect()
    const limit = this.capabilities().max_pending
    if (this.pending.size >= limit) {
      throw new BackendError({ code: 'queue_full', message: `EBUSY: QUEUE_FULL, ${this.pending.size} RPCs already waiting`, retryable: true })
    }
    const id = this.nextId++
    const payload: Record<string, unknown> = { ...(params as Record<string, unknown>), id, method }
    if (opts.repositoryId) payload.repository_id = opts.repositoryId
    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) {
          reject(new BackendError({ code: 'timeout', message: `RPC timeout: ${method}`, retryable: false }))
        }
      }, opts.deadlineMs ?? 170_000)
      this.pending.set(id, { resolve, reject, timer })
      try {
        this.socket!.send(JSON.stringify(payload))
      } catch (error) {
        this.pending.delete(id)
        clearTimeout(timer)
        reject(toBackendError(error))
      }
      if (opts.signal) {
        const onAbort = () => {
          this.pending.delete(id)
          clearTimeout(timer)
          try {
            this.socket!.send(JSON.stringify({ id: this.nextId++, method: 'cancel', request_id: id }))
          } catch {}
          reject(new BackendError({ code: 'cancelled', message: 'Operation cancelled', retryable: true }))
        }
        if (opts.signal.aborted) onAbort()
        else opts.signal.addEventListener('abort', onAbort, { once: true })
      }
    })
  }

  cancel(requestId: string | number): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return
    const id = typeof requestId === 'string' ? parseInt(requestId, 10) : requestId
    if (Number.isNaN(id)) return
    this.socket.send(JSON.stringify({ id: this.nextId++, method: 'cancel', request_id: id }))
  }

  subscribe(event: string, listener: (...args: unknown[]) => void): () => void {
    let set = this.listeners.get(event)
    if (!set) {
      set = new Set()
      this.listeners.set(event, set)
    }
    set.add(listener)
    return () => {
      set!.delete(listener)
    }
  }

  private dispatch(event: string, payload: unknown): void {
    for (const fn of this.listeners.get(event) ?? []) {
      try {
        fn(payload)
      } catch (error) {
        console.error(`[backend] listener failed for ${event}`, error)
      }
    }
  }

  close(): void {
    this.closed = true
    this.socket?.close()
  }

  // AsyncIterable for snapshot/job streams (not used by the bridge today
  // beyond `repository_changed`; kept for parity with the Wry dispatcher).
  [Symbol.asyncIterator]() {
    return this
  }
  async next(): Promise<IteratorResult<unknown>> {
    if (this.queue.length) return { value: this.queue.shift(), done: false }
    if (this.closed) return { value: undefined, done: true }
    const value = await new Promise<unknown>(resolve => this.waiters.push(resolve))
    return { value, done: false }
  }
}

// ---------------------------------------------------------------------------
// Client facade — picks transport, exposes typed calls
// ---------------------------------------------------------------------------

export class BackendClient {
  private wry: WryTransport | null = null
  private ws: WebSocketTransport | null = null
  private transport: 'wry' | 'ws' | 'none' = 'none'
  private readyPromise: Promise<ReadyPayload> | null = null

  constructor() {
    const w = (window as any).__desktop as WryBridge | undefined
    if (w && typeof w.invoke === 'function' && typeof w.on === 'function') {
      this.wry = new WryTransport(w)
      this.transport = 'wry'
    } else if (window.location.hash.includes('token=')) {
      this.ws = new WebSocketTransport()
      this.transport = 'ws'
    }
  }

  get transportKind(): 'wry' | 'ws' | 'none' {
    return this.transport
  }

  get isReady(): boolean {
    return this.transport === 'wry' || (this.ws?.ready ?? false)
  }

  async waitReady(): Promise<ReadyPayload> {
    if (this.transport === 'wry') {
      return { ready: true, version: 3, capabilities: this.wry!.capabilities() }
    }
    if (this.transport === 'ws') {
      if (!this.readyPromise) this.readyPromise = this.ws!.connect()
      return this.readyPromise
    }
    throw new BackendError({ code: 'no_transport', message: 'no backend transport attached (run via desktop-rs serve or Tao/Wry shell)', retryable: false })
  }

  capabilities(): Capabilities {
    if (this.transport === 'wry') return this.wry!.capabilities()
    if (this.transport === 'ws') return this.ws!.capabilities()
    return { cancel: true, max_pending: 32 }
  }

  async request<T = unknown>(method: string, params: unknown, opts: BackendRequestOptions = {}): Promise<T> {
    if (this.transport === 'wry') return this.wry!.request(method, params, opts) as Promise<T>
    if (this.transport === 'ws') return this.ws!.request(method, params, opts) as Promise<T>
    throw new BackendError({ code: 'no_transport', message: 'no backend transport attached', retryable: false })
  }

  cancel(requestId: string): void {
    if (this.transport === 'wry') this.wry!.cancel(requestId)
    else if (this.transport === 'ws') this.ws!.cancel(requestId)
  }

  /** Snapshot of current repository (ws) or health/capabilities (wry). */
  async snapshot(opts: BackendRequestOptions = {}): Promise<unknown> {
    if (this.transport === 'ws') return this.request('snapshot', {}, opts)
    if (this.transport === 'wry') {
      const health = (window as any).__desktop as { health?: () => unknown } | undefined
      if (typeof health?.health === 'function') return health.health()
      return { version: 3, capabilities: this.wry!.capabilities() }
    }
    throw new BackendError({ code: 'no_transport', message: 'no backend transport attached', retryable: false })
  }
}

// Singleton — shared by all stores; renderer subscribes/proxies once.
let singleton: BackendClient | null = null
export function backend(): BackendClient {
  if (!singleton) singleton = new BackendClient()
  return singleton
}
