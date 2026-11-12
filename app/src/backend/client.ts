/**
 * Typed frontend client for Rust application operations. Owns transports (Wry via
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
 * Wry exposes typed repository reads and explicit trust requests; renderer
 * workflows still use existing shims until the remaining operations migrate.
 */

import {
  Capabilities,
  IBackendBranch,
  IBackendBranchRef,
  DetailedStatusRecord,
  IBackendError,
  ReadyPayload,
} from './contracts'
import { randomUUID } from '../web/random-uuid'
import { isRecord } from '../lib/is-record'

export interface BackendRequestOptions {
  readonly repositoryId?: string
  readonly accountId?: string
  readonly deadlineMs?: number
  readonly signal?: AbortSignal
}

export type BackendGitPath = string | { readonly bytes: ReadonlyArray<number> }

export interface ICopilotAccount {
  readonly host: string
  readonly login: string
  readonly id: number
}

export interface ICopilotCommitOptions {
  readonly remove_paths?: ReadonlyArray<BackendGitPath>
  readonly paths?: ReadonlyArray<BackendGitPath>
  readonly system_prompt?: string
  readonly source?: 'staged' | 'working_tree'
  readonly partial_selections?: ReadonlyArray<{
    readonly path: BackendGitPath
    readonly original_path?: BackendGitPath
    readonly basis: string
    readonly lines: ReadonlyArray<number>
  }>
  readonly amend_head?: string
  readonly expected_head?: {
    readonly ref: string | null
    readonly sha: string | null
  }
  readonly rule_descriptions?: ReadonlyArray<string>
  readonly account?: ICopilotAccount
  readonly model?: string
  readonly reasoning_effort?: 'low' | 'medium' | 'high' | 'xhigh'
  readonly timeout_ms?: number
  readonly provider?: {
    readonly type: 'openai' | 'azure' | 'anthropic'
    readonly baseUrl: string
    readonly wireApi?: 'completions' | 'responses'
    readonly apiKey?: string
    readonly bearerToken?: string
    readonly azure?: { readonly apiVersion: string }
  }
}

export interface ICopilotCommitDiff {
  readonly path: BackendGitPath
  readonly basis: string
  readonly kind: string
  readonly can_select: boolean
  readonly hunks: ReadonlyArray<{
    readonly old_start: number
    readonly old_count: number
    readonly new_start: number
    readonly new_count: number
    readonly lines: ReadonlyArray<{
      readonly id: number
      readonly kind: string
      readonly text: string
      readonly old_line: number | null
      readonly new_line: number | null
      readonly no_newline: boolean
    }>
  }>
}

function isBackendGitPath(value: unknown): value is BackendGitPath {
  return (
    typeof value === 'string' ||
    (isRecord(value) &&
      Array.isArray(value.bytes) &&
      value.bytes.every(
        byte => Number.isInteger(byte) && byte >= 0 && byte <= 255
      ))
  )
}

function isCopilotCommitDiff(value: unknown): value is ICopilotCommitDiff {
  const count = (entry: unknown) =>
    typeof entry === 'number' && Number.isSafeInteger(entry) && entry >= 0
  const lineNumber = (entry: unknown) => entry === null || count(entry)
  return (
    isRecord(value) &&
    isBackendGitPath(value.path) &&
    typeof value.basis === 'string' &&
    value.basis.length > 0 &&
    typeof value.kind === 'string' &&
    typeof value.can_select === 'boolean' &&
    Array.isArray(value.hunks) &&
    value.hunks.every(
      hunk =>
        isRecord(hunk) &&
        count(hunk.old_start) &&
        count(hunk.old_count) &&
        count(hunk.new_start) &&
        count(hunk.new_count) &&
        Array.isArray(hunk.lines) &&
        hunk.lines.every(
          line =>
            isRecord(line) &&
            count(line.id) &&
            (line.kind === 'context' ||
              line.kind === 'added' ||
              line.kind === 'deleted') &&
            typeof line.text === 'string' &&
            lineNumber(line.old_line) &&
            lineNumber(line.new_line) &&
            typeof line.no_newline === 'boolean'
        )
    )
  )
}

function copilotDeadline(
  timeout: number | undefined,
  options: BackendRequestOptions
): BackendRequestOptions {
  return options.deadlineMs === undefined &&
    timeout !== undefined &&
    Number.isInteger(timeout) &&
    timeout >= 1 &&
    timeout <= 300_000
    ? { ...options, deadlineMs: Math.max(170_000, timeout + 30_000) }
    : options
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
  const anyValue = value as Partial<IBackendError> & {
    code?: unknown
    message?: unknown
    error_code?: unknown
    error?: unknown
  }
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
    retryable:
      code === 'cancelled' ||
      code === 'queue_full' ||
      anyValue?.retryable === true,
    jobId: typeof anyValue?.jobId === 'string' ? anyValue.jobId : undefined,
    recoveryId:
      typeof anyValue?.recoveryId === 'string'
        ? anyValue.recoveryId
        : undefined,
    details: anyValue?.details as IBackendError['details'],
  })
}

/** Profile-derived pending limit. The Rust limits are the authority; the
 *  shim exposes `__health` with `pending_promises`. We default to the
 *  normal profile (128) and honour `low-memory` (64) when presented. */
function pendingLimitFromHealth(): number {
  const desktop = (window as any)?.__desktop as
    | { pendingPromises?: number }
    | undefined
  if (typeof desktop?.pendingPromises === 'number')
    return desktop.pendingPromises
  const profile =
    typeof (window as any)?.__desktopEnv?.DESKTOP_RESOURCE_PROFILE === 'string'
      ? (window as any).__desktopEnv.DESKTOP_RESOURCE_PROFILE
      : typeof (window as any)?.DESKTOP_RESOURCE_PROFILE === 'string'
      ? (window as any).DESKTOP_RESOURCE_PROFILE
      : undefined
  return profile === 'low-memory' ? 64 : 128
}

type WryBridge = {
  readonly __transport?: string
  supportsCancellation?: () => boolean
  invoke: (channel: string, ...args: unknown[]) => Promise<unknown>
  send: (channel: string, ...args: unknown[]) => void
  on: (
    channel: string,
    listener: (event: unknown, ...args: unknown[]) => void
  ) => unknown
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
    return {
      cancel: this.bridge.supportsCancellation?.() ?? true,
      max_pending: pendingLimitFromHealth(),
    }
  }

  async request(
    method: string,
    params: unknown,
    opts: BackendRequestOptions
  ): Promise<unknown> {
    if (opts.signal?.aborted) {
      throw new BackendError({
        code: 'cancelled',
        message: 'Operation cancelled before start',
        retryable: true,
      })
    }
    if (method.startsWith('copilot_')) {
      throw new BackendError({
        code: 'unsupported',
        message:
          'Use a repository-scoped Copilot operation in the desktop shell',
        retryable: false,
      })
    }
    if (
      opts.repositoryId !== undefined ||
      opts.accountId !== undefined ||
      (method !== 'read' &&
        method !== 'copilot' &&
        (opts.signal !== undefined || opts.deadlineMs !== undefined))
    ) {
      throw new BackendError({
        code: 'unsupported',
        message: 'Wry backend request options are not implemented',
        retryable: false,
      })
    }
    const channel = `desktop.backend.${method}`
    if (method === 'read' || method === 'copilot') {
      if (opts.signal && !this.capabilities().cancel) {
        throw new BackendError({
          code: 'unsupported',
          message: 'Cancellation requires the native IPC transport',
          retryable: false,
        })
      }
      const maximum = method === 'read' ? 60_000 : 330_000
      const deadline = opts.deadlineMs ?? (method === 'read' ? 60_000 : 170_000)
      if (!Number.isInteger(deadline) || deadline < 1 || deadline > maximum) {
        throw new BackendError({
          code: 'invalid_request',
          message: `deadlineMs must be between 1 and ${maximum}`,
          retryable: false,
        })
      }
      const requestId = randomUUID()
      return new Promise((resolve, reject) => {
        let settled = false
        let timer: ReturnType<typeof setTimeout>
        const finish = (action: () => void) => {
          if (settled) return
          settled = true
          clearTimeout(timer)
          opts.signal?.removeEventListener('abort', abort)
          action()
        }
        const cancel = (code: string, message: string) =>
          finish(() => {
            try {
              if (this.capabilities().cancel) this.cancel(requestId)
            } finally {
              reject(
                new BackendError({
                  code,
                  message,
                  retryable: method === 'read',
                })
              )
            }
          })
        const abort = () => cancel('cancelled', 'Backend operation cancelled')
        opts.signal?.addEventListener('abort', abort, { once: true })
        timer = setTimeout(
          () => cancel('ETIMEDOUT', 'Backend operation timed out'),
          deadline
        )
        try {
          this.bridge
            .invoke(channel, {
              ...(params as object),
              request_id: requestId,
              deadline_ms: deadline,
            })
            .then(
              value => finish(() => resolve(value)),
              error => finish(() => reject(toBackendError(error)))
            )
        } catch (error) {
          finish(() => reject(toBackendError(error)))
        }
      })
    }
    try {
      const value = await this.bridge.invoke(channel, params)
      return value
    } catch (error) {
      throw toBackendError(error)
    }
  }

  subscribe(event: string, listener: (...args: unknown[]) => void): () => void {
    const wrapped = (_event: unknown, ...args: unknown[]) => listener(...args)
    this.bridge.on(event, wrapped)
    return () => this.bridge.off?.(event, wrapped)
  }

  cancel(requestId: string): void {
    if (!this.capabilities().cancel) {
      throw new BackendError({
        code: 'unsupported',
        message: 'Cancellation requires the native IPC transport',
        retryable: false,
      })
    }
    this.bridge.send('desktop.backend.cancel', { request_id: requestId })
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

function disconnectedError(): BackendError {
  return new BackendError({
    code: 'disconnected',
    message: 'Bridge socket closed',
    retryable: true,
  })
}

function disconnectedInFlightError(): BackendError {
  return new BackendError({
    code: 'disconnected',
    message:
      'Bridge disconnected; the operation may have completed. Inspect state before retrying.',
    retryable: false,
  })
}

function bridgeToken(): string | null {
  const fragment = new URLSearchParams(window.location.hash.slice(1))
  if (fragment.has('token')) {
    const token = fragment.get('token')
    if (token) return token
    try {
      sessionStorage.removeItem('desktopBridgeToken')
    } catch {}
    return null
  }
  try {
    return sessionStorage.getItem('desktopBridgeToken')
  } catch {
    return null
  }
}

class WebSocketTransport {
  private socket: WebSocket | null = null
  private wsReady: Promise<ReadyPayload> | null = null
  private pending = new Map<
    number,
    {
      resolve: (v: unknown) => void
      reject: (e: unknown) => void
      cleanup: () => void
    }
  >()
  private listeners = new Map<string, Set<(...args: unknown[]) => void>>()
  private nextId = 1
  private readonly token: string
  private _ready: ReadyPayload | null = null
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private closed = false
  private terminalError: BackendError | null = null
  private readyWaiters: Array<{
    resolve: (v: ReadyPayload) => void
    reject: (e: unknown) => void
  }> = []

  constructor(token: string) {
    this.token = token
    try {
      sessionStorage.setItem('desktopBridgeToken', token)
      const fragment = new URLSearchParams(window.location.hash.slice(1))
      if (fragment.has('token')) {
        fragment.delete('token')
        const rest = fragment.toString()
        window.history.replaceState(
          window.history.state,
          '',
          `${window.location.pathname}${window.location.search}${
            rest ? `#${rest}` : ''
          }`
        )
      }
    } catch {
      // Storage is optional while the token is still present in the URL.
    }
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
    if (this._ready) return Promise.resolve(this._ready)
    if (this.closed) {
      return Promise.reject(this.terminalError ?? disconnectedError())
    }
    if (this.wsReady) return this.wsReady
    this.wsReady = new Promise<ReadyPayload>((resolve, reject) => {
      this.readyWaiters.push({ resolve, reject })
    })
    const ready = this.wsReady
    if (!this.socket && !this.reconnectTimer) this.open()
    return ready
  }

  private open(): void {
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    const url = `${scheme}//${window.location.host}/ws`
    let socket: WebSocket
    try {
      socket = new WebSocket(url)
    } catch {
      this.failReady(disconnectedError())
      this.scheduleReconnect()
      return
    }
    this.socket = socket
    const handshakeTimer = setTimeout(() => {
      if (this.socket === socket && !this._ready && !this.closed) socket.close()
    }, 15_000)
    socket.onmessage = event => {
      if (this.socket !== socket || this.closed) return
      try {
        const data = JSON.parse(event.data) as BridgeRpcReply & {
          ready?: boolean
          version?: number
          capabilities?: Capabilities & { cancel: boolean; max_pending: number }
        }
        if (
          !data ||
          typeof data !== 'object' ||
          (!this._ready && data.ready !== true)
        ) {
          throw new Error('invalid bridge frame')
        }
        if (data.ready === true) {
          if (
            data.version !== 3 ||
            typeof data.capabilities?.cancel !== 'boolean' ||
            !Number.isSafeInteger(data.capabilities?.max_pending) ||
            data.capabilities!.max_pending < 1 ||
            data.capabilities!.max_pending > 4096
          ) {
            throw new Error('unsupported bridge handshake')
          }
          clearTimeout(handshakeTimer)
          this._ready = {
            ready: true,
            version: 3,
            capabilities: {
              cancel: data.capabilities.cancel,
              max_pending: data.capabilities.max_pending,
            },
          }
          this.flushReady()
          return
        }
        if (typeof data.id === 'number') {
          const entry = this.pending.get(data.id)
          if (entry) {
            this.pending.delete(data.id)
            entry.cleanup()
            if (data.error !== undefined) {
              entry.reject(
                new BackendError({
                  code: data.error_code ?? 'internal',
                  message: data.error,
                  retryable: data.error_code === 'queue_full',
                })
              )
            } else {
              entry.resolve(data.result)
            }
          }
          return
        }
        if (data.event) {
          this.dispatch(
            data.event,
            'snapshot' in data
              ? data.snapshot
              : 'preferences' in data
              ? data.preferences
              : data
          )
        }
      } catch {
        clearTimeout(handshakeTimer)
        this.terminalError = new BackendError({
          code: 'protocol',
          message:
            'Invalid bridge message; inspect state before retrying an operation',
          retryable: false,
        })
        this.closed = true
        this.failReady(this.terminalError)
        this.rejectPending(this.terminalError)
        socket.close()
      }
    }
    socket.onopen = () => {
      if (this.socket !== socket || this.closed) return
      socket.send(JSON.stringify({ token: this.token }))
    }
    socket.onclose = event => {
      if (this.socket !== socket) return
      clearTimeout(handshakeTimer)
      this.socket = null
      this._ready = null
      if (event.code === 1008 && !this.closed) {
        this.closed = true
        this.terminalError = new BackendError({
          code: 'unauthorized',
          message: 'Bridge rejected the session token',
          retryable: false,
        })
        try {
          if (sessionStorage.getItem('desktopBridgeToken') === this.token)
            sessionStorage.removeItem('desktopBridgeToken')
        } catch {}
      }
      const error = this.terminalError ?? disconnectedError()
      this.failReady(error)
      this.rejectPending(this.terminalError ?? disconnectedInFlightError())
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
    this.wsReady = null
    for (const w of this.readyWaiters.splice(0)) w.reject(error)
  }

  private rejectPending(error: unknown): void {
    for (const entry of this.pending.values()) {
      entry.cleanup()
      entry.reject(error)
    }
    this.pending.clear()
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer || this.closed) return
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      if (!this.socket) this.open()
    }, 1000)
  }

  async request(
    method: string,
    params: unknown,
    opts: BackendRequestOptions
  ): Promise<unknown> {
    if (opts.signal?.aborted) {
      throw new BackendError({
        code: 'cancelled',
        message: 'Operation cancelled before start',
        retryable: true,
      })
    }
    if (opts.accountId !== undefined) {
      throw new BackendError({
        code: 'unsupported',
        message: 'Account-scoped bridge requests are not implemented',
        retryable: false,
      })
    }
    const deadlineMs = opts.deadlineMs ?? 170_000
    if (
      !Number.isFinite(deadlineMs) ||
      deadlineMs <= 0 ||
      deadlineMs > 2_147_483_647
    ) {
      throw new BackendError({
        code: 'invalid_deadline',
        message: 'Invalid bridge request deadline',
        retryable: false,
      })
    }
    const deadline = Date.now() + deadlineMs
    if (!this._ready) await this.waitForReady(opts.signal, deadlineMs)
    if (opts.signal?.aborted) {
      throw new BackendError({
        code: 'cancelled',
        message: 'Operation cancelled before start',
        retryable: true,
      })
    }
    const socket = this.socket
    if (!this._ready || !socket || socket.readyState !== WebSocket.OPEN)
      throw disconnectedError()
    if (Date.now() >= deadline) {
      throw new BackendError({
        code: 'timeout',
        message: 'Bridge request deadline exceeded before start',
        retryable: true,
      })
    }
    const limit = this.capabilities().max_pending
    if (this.pending.size >= limit) {
      throw new BackendError({
        code: 'queue_full',
        message: `EBUSY: QUEUE_FULL, ${this.pending.size} RPCs already waiting`,
        retryable: true,
      })
    }
    const id = this.nextId++
    const payload: Record<string, unknown> = {
      ...(params as Record<string, unknown>),
      id,
      method,
    }
    if (opts.repositoryId !== undefined)
      payload.repository_id = opts.repositoryId
    return new Promise<unknown>((resolve, reject) => {
      let onAbort: (() => void) | undefined
      const cleanup = () => {
        clearTimeout(timer)
        if (onAbort) opts.signal?.removeEventListener('abort', onAbort)
      }
      const rejectAndCancel = (error: BackendError) => {
        if (!this.pending.delete(id)) return
        cleanup()
        if (this.capabilities().cancel) this.cancel(id)
        reject(error)
      }
      const timer = setTimeout(() => {
        rejectAndCancel(
          new BackendError({
            code: 'timeout',
            message: `RPC timeout: ${method}; inspect state before retrying a mutation`,
            retryable: false,
          })
        )
      }, Math.max(0, deadline - Date.now()))
      this.pending.set(id, { resolve, reject, cleanup })
      if (opts.signal) {
        onAbort = () =>
          rejectAndCancel(
            new BackendError({
              code: 'cancelled',
              message:
                'Cancellation requested; inspect state before retrying a mutation',
              retryable: false,
            })
          )
        opts.signal.addEventListener('abort', onAbort, { once: true })
      }
      try {
        socket.send(JSON.stringify(payload))
      } catch (error) {
        this.pending.delete(id)
        cleanup()
        reject(toBackendError(error))
      }
      if (opts.signal?.aborted) onAbort?.()
    })
  }

  private waitForReady(
    signal: AbortSignal | undefined,
    timeoutMs: number
  ): Promise<ReadyPayload> {
    return new Promise((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', abort)
      }
      const abort = () => {
        cleanup()
        reject(
          new BackendError({
            code: 'cancelled',
            message: 'Operation cancelled before start',
            retryable: true,
          })
        )
      }
      const timer = setTimeout(() => {
        cleanup()
        reject(
          new BackendError({
            code: 'timeout',
            message: 'Bridge connection deadline exceeded before start',
            retryable: true,
          })
        )
      }, timeoutMs)
      signal?.addEventListener('abort', abort, { once: true })
      this.connect().then(
        value => {
          cleanup()
          resolve(value)
        },
        error => {
          cleanup()
          reject(error)
        }
      )
      if (signal?.aborted) abort()
    })
  }

  cancel(requestId: string | number): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return
    const id = typeof requestId === 'string' ? Number(requestId) : requestId
    if (!Number.isSafeInteger(id) || id < 1) return
    try {
      this.socket.send(
        JSON.stringify({ id: this.nextId++, method: 'cancel', request_id: id })
      )
    } catch {
      // The socket may close between the readiness check and send.
    }
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
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this._ready = null
    const error = this.terminalError ?? disconnectedError()
    this.failReady(error)
    this.rejectPending(this.terminalError ?? disconnectedInFlightError())
    this.socket?.close()
  }
}

// ---------------------------------------------------------------------------
// Client facade — picks transport, exposes typed calls
// ---------------------------------------------------------------------------

export class BackendClient {
  private wry: WryTransport | null = null
  private ws: WebSocketTransport | null = null
  private transport: 'wry' | 'ws' | 'none' = 'none'

  constructor() {
    const w = (window as any).__desktop as WryBridge | undefined
    if (
      w &&
      w.__transport === undefined &&
      typeof w.invoke === 'function' &&
      typeof w.on === 'function'
    ) {
      this.wry = new WryTransport(w)
      this.transport = 'wry'
    } else {
      const token = bridgeToken()
      if (token) {
        this.ws = new WebSocketTransport(token)
        this.transport = 'ws'
      }
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
      return this.ws!.connect()
    }
    throw new BackendError({
      code: 'no_transport',
      message:
        'no backend transport attached (run via desktop-rs serve or Tao/Wry shell)',
      retryable: false,
    })
  }

  capabilities(): Capabilities {
    if (this.transport === 'wry') return this.wry!.capabilities()
    if (this.transport === 'ws') return this.ws!.capabilities()
    return { cancel: false, max_pending: 32 }
  }

  async request<T = unknown>(
    method: string,
    params: unknown,
    opts: BackendRequestOptions = {}
  ): Promise<T> {
    if (this.transport === 'wry')
      return this.wry!.request(method, params, opts) as Promise<T>
    if (this.transport === 'ws')
      return this.ws!.request(method, params, opts) as Promise<T>
    throw new BackendError({
      code: 'no_transport',
      message: 'no backend transport attached',
      retryable: false,
    })
  }

  cancel(requestId: string): void {
    if (this.transport === 'wry') this.wry!.cancel(requestId)
    else if (this.transport === 'ws') this.ws!.cancel(requestId)
  }

  subscribe(event: string, listener: (...args: unknown[]) => void): () => void {
    if (this.transport === 'wry') return this.wry!.subscribe(event, listener)
    if (this.transport === 'ws') return this.ws!.subscribe(event, listener)
    return () => {}
  }

  close(): void {
    this.ws?.close()
  }

  /** Snapshot of current repository (ws) or health/capabilities (wry). */
  async snapshot(opts: BackendRequestOptions = {}): Promise<unknown> {
    if (this.transport === 'ws') return this.request('snapshot', {}, opts)
    if (this.transport === 'wry') {
      const health = (window as any).__desktop as
        | { health?: () => unknown }
        | undefined
      if (typeof health?.health === 'function') return health.health()
      return { version: 3, capabilities: this.wry!.capabilities() }
    }
    throw new BackendError({
      code: 'no_transport',
      message: 'no backend transport attached',
      retryable: false,
    })
  }

  /** Read a specific repository through the native shell's Rust core. */
  async readRepositoryState(
    repositoryPath: string,
    method: 'snapshot' | 'status'
  ): Promise<unknown> {
    if (this.transport !== 'wry') {
      throw new BackendError({
        code: 'unsupported',
        message: 'repository-scoped Wry reads require the desktop shell',
        retryable: false,
      })
    }
    return this.wry!.request(
      'read',
      { repository_path: repositoryPath, method },
      {}
    )
  }

  private async repositoryRead(
    repositoryPath: string,
    method: string,
    params: Record<string, unknown>,
    options: BackendRequestOptions = {}
  ): Promise<unknown> {
    if (this.transport === 'wry') {
      return this.wry!.request(
        'read',
        { ...params, repository_path: repositoryPath, method },
        options
      )
    } else if (this.transport === 'ws') {
      const started = Date.now()
      const binding = await this.request(
        'repository_binding',
        { path: repositoryPath },
        {
          signal: options.signal,
          deadlineMs: options.deadlineMs,
        }
      )
      if (!isRecord(binding) || typeof binding.root_id !== 'string') {
        throw new BackendError({
          code: 'repository_changed',
          message:
            'Selected repository does not match the requested repository',
          retryable: true,
        })
      }
      const remaining = (options.deadlineMs ?? 170_000) - (Date.now() - started)
      if (remaining <= 0) {
        throw new BackendError({
          code: 'timeout',
          message: 'Bridge request deadline exceeded before repository read',
          retryable: true,
        })
      }
      return this.request(method, params, {
        ...options,
        deadlineMs: remaining,
        repositoryId: binding.root_id,
      })
    } else {
      throw new BackendError({
        code: 'unsupported',
        message: 'repository-scoped reads require a backend transport',
        retryable: false,
      })
    }
  }

  /** Classify a directory before it has been selected as a repository. */
  async repositoryType(
    path: string
  ): Promise<
    | { kind: 'bare' | 'missing' }
    | { kind: 'regular'; topLevelWorkingDirectory: string; gitDir: string }
    | { kind: 'unsafe'; path: string }
  > {
    const value =
      this.transport === 'wry'
        ? await this.repositoryRead(path, 'repository_type', {})
        : await this.request('repository_type', { path })
    if (isRecord(value)) {
      if (value.kind === 'bare' || value.kind === 'missing')
        return { kind: value.kind }
      if (value.kind === 'unsafe' && typeof value.path === 'string')
        return { kind: 'unsafe', path: value.path }
      if (
        value.kind === 'regular' &&
        typeof value.topLevelWorkingDirectory === 'string' &&
        typeof value.gitDir === 'string'
      ) {
        return {
          kind: 'regular',
          topLevelWorkingDirectory: value.topLevelWorkingDirectory,
          gitDir: value.gitDir,
        }
      }
    }
    throw new BackendError({
      code: 'invalid_response',
      message: 'Invalid repository type response',
      retryable: false,
    })
  }

  /** Check a directory independently of the currently selected repository. */
  async directoryExists(path: string): Promise<boolean> {
    const value =
      this.transport === 'wry'
        ? await this.repositoryRead(path, 'directory_exists', {})
        : await this.request('directory_exists', { path })
    if (typeof value !== 'boolean') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid directory check response',
        retryable: false,
      })
    }
    return value
  }

  /** Check for a file or directory without selecting a repository. */
  async pathExists(path: string): Promise<boolean> {
    const value =
      this.transport === 'wry'
        ? await this.repositoryRead(path, 'path_exists', {})
        : await this.request('path_exists', { path })
    if (typeof value !== 'boolean') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid path check response',
        retryable: false,
      })
    }
    return value
  }

  /** Resolve an existing path or a prospective destination through Rust. */
  async canonicalMutationPath(path: string): Promise<string> {
    const value =
      this.transport === 'wry'
        ? await this.repositoryRead(path, 'canonical_mutation_path', {})
        : await this.request('canonical_mutation_path', { path })
    if (typeof value !== 'string') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid canonical path response',
        retryable: false,
      })
    }
    return value
  }

  /** Trust one repository root after the user confirms the exception. */
  async trustRepository(path: string): Promise<void> {
    const value =
      this.transport === 'wry'
        ? await this.repositoryRead(path, 'trust_repository', {})
        : await this.request('trust_repository', { path })
    if (!isRecord(value) || value.trusted !== true) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid repository trust response',
        retryable: false,
      })
    }
  }

  /** Fetch Git's disambiguated branches for one explicit repository. */
  async repositoryBranches(
    repositoryPath: string,
    prefixes: ReadonlyArray<string>
  ): Promise<ReadonlyArray<IBackendBranch>> {
    const value = await this.repositoryRead(
      repositoryPath,
      'branch_enumeration',
      { prefixes }
    )
    if (
      !Array.isArray(value) ||
      !value.every(
        branch =>
          branch !== null &&
          typeof branch === 'object' &&
          typeof branch.ref === 'string' &&
          typeof branch.name === 'string' &&
          (branch.upstream === null || typeof branch.upstream === 'string') &&
          (branch.upstream_ref === null ||
            typeof branch.upstream_ref === 'string') &&
          typeof branch.current === 'boolean' &&
          typeof branch.oid === 'string'
      )
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid branch enumeration response',
        retryable: false,
      })
    }
    return value
  }

  /** Fetch local refs merged into a selected branch. */
  async mergedRepositoryBranches(
    repositoryPath: string,
    branch: string
  ): Promise<ReadonlyArray<IBackendBranchRef>> {
    const value = await this.repositoryRead(repositoryPath, 'branch_merged', {
      branch,
    })
    if (
      !Array.isArray(value) ||
      !value.every(
        item =>
          item !== null &&
          typeof item === 'object' &&
          typeof item.ref === 'string' &&
          typeof item.oid === 'string'
      )
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid merged branches response',
        retryable: false,
      })
    }
    return value
  }

  async branchesPointingAt(
    repositoryPath: string,
    commitish: string
  ): Promise<ReadonlyArray<string> | null> {
    const value = await this.repositoryRead(
      repositoryPath,
      'branch_points_at',
      { commitish }
    )
    if (
      value !== null &&
      (!Array.isArray(value) || !value.every(item => typeof item === 'string'))
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid branches pointing at response',
        retryable: false,
      })
    }
    return value
  }

  async repositoryAheadBehind(
    repositoryPath: string,
    range: string
  ): Promise<{ readonly ahead: number; readonly behind: number } | null> {
    const value = await this.repositoryRead(repositoryPath, 'ahead_behind', {
      range,
    })
    if (value === null) return null
    if (
      !isRecord(value) ||
      typeof value.ahead !== 'number' ||
      typeof value.behind !== 'number' ||
      !Number.isSafeInteger(value.ahead) ||
      !Number.isSafeInteger(value.behind) ||
      value.ahead < 0 ||
      value.behind < 0
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid ahead/behind response',
        retryable: false,
      })
    }
    return { ahead: value.ahead, behind: value.behind }
  }

  /** Fetch the renderer's rebase-ordered revision list. */
  async repositoryRevisionList(
    repositoryPath: string,
    range: string
  ): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, 'revision_list', {
      range,
    })
    if (value !== null && typeof value !== 'string') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid revision list response',
        retryable: false,
      })
    }
    return value
  }

  async repositoryHasMergeCommits(
    repositoryPath: string,
    range: string
  ): Promise<boolean> {
    const value = await this.repositoryRead(
      repositoryPath,
      'has_merge_commits',
      { range }
    )
    if (typeof value !== 'boolean') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid merge commit response',
        retryable: false,
      })
    }
    return value
  }

  async repositoryStatusContext(repositoryPath: string): Promise<{
    readonly mergeHeadFound: boolean
    readonly squashMsgFound: boolean
    readonly isCherryPickingHeadFound: boolean
    readonly rebaseInternalState: {
      readonly originalBranchTip: string
      readonly targetBranch: string
      readonly baseBranchTip: string
    } | null
  }> {
    const value = await this.repositoryRead(
      repositoryPath,
      'status_context',
      {}
    )
    if (
      !isRecord(value) ||
      typeof value.mergeHeadFound !== 'boolean' ||
      typeof value.squashMsgFound !== 'boolean' ||
      typeof value.isCherryPickingHeadFound !== 'boolean'
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid repository status context response',
        retryable: false,
      })
    }
    const rebase = value.rebaseInternalState
    let rebaseInternalState = null
    if (rebase !== null) {
      if (
        !isRecord(rebase) ||
        typeof rebase.originalBranchTip !== 'string' ||
        typeof rebase.targetBranch !== 'string' ||
        typeof rebase.baseBranchTip !== 'string'
      ) {
        throw new BackendError({
          code: 'invalid_response',
          message: 'Invalid repository rebase context response',
          retryable: false,
        })
      }
      rebaseInternalState = {
        originalBranchTip: rebase.originalBranchTip,
        targetBranch: rebase.targetBranch,
        baseBranchTip: rebase.baseBranchTip,
      }
    }
    return {
      mergeHeadFound: value.mergeHeadFound,
      squashMsgFound: value.squashMsgFound,
      isCherryPickingHeadFound: value.isCherryPickingHeadFound,
      rebaseInternalState,
    }
  }

  async repositoryConflictDetails(
    repositoryPath: string,
    kind: 'merge' | 'rebase' | 'working'
  ): Promise<{
    readonly check: string
    readonly numstat: string
    readonly attributes: string
  }> {
    const value = await this.repositoryRead(
      repositoryPath,
      'status_conflict_details',
      { kind }
    )
    if (
      !isRecord(value) ||
      typeof value.check !== 'string' ||
      typeof value.numstat !== 'string' ||
      typeof value.attributes !== 'string'
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid conflict details response',
        retryable: false,
      })
    }
    return {
      check: value.check,
      numstat: value.numstat,
      attributes: value.attributes,
    }
  }

  /** Fetch byte-preserving status records for one explicit repository. */
  async detailedRepositoryStatus(
    repositoryPath: string,
    includeUntracked = true,
    options: BackendRequestOptions = {}
  ): Promise<ReadonlyArray<DetailedStatusRecord>> {
    return this.repositoryRead(
      repositoryPath,
      'status_detailed',
      {
        include_untracked: includeUntracked,
      },
      options
    ) as Promise<ReadonlyArray<DetailedStatusRecord>>
  }

  // ---------------------------------------------------------------------------
  // Copilot — typed operations exposed by the Rust service. The existing
  // renderer assistant flows still use the SDK until their account, model,
  // provider, rules and diff-selection behavior has migrated.
  // ---------------------------------------------------------------------------

  copilotProbe(
    runtimeRoot?: string
  ): Promise<{ available: boolean; reason: string; sdk_version: string }> {
    return this.request(
      'copilot_probe',
      runtimeRoot ? { runtime_root: runtimeRoot } : {}
    )
  }

  copilotCreateSession(
    opts: {
      model?: string
      reasoning_effort?: string
      account?: ICopilotAccount
    } = {}
  ): Promise<{ id: string; connection_id: string; model: string }> {
    return this.request('copilot_create_session', {
      model: opts.model,
      reasoning_effort: opts.reasoning_effort,
      account: opts.account,
    })
  }

  copilotSendMessage(
    sessionId: string,
    message: string,
    opts: BackendRequestOptions = {}
  ): Promise<{ content: string; session_id: string }> {
    return this.request(
      'copilot_send_message',
      { session_id: sessionId, message },
      opts
    )
  }

  copilotCloseSession(
    sessionId: string,
    opts: BackendRequestOptions = {}
  ): Promise<{ closed: boolean }> {
    return this.request(
      'copilot_close_session',
      { session_id: sessionId },
      opts
    )
  }

  copilotCommitDiffDetail(
    path: BackendGitPath,
    opts: BackendRequestOptions = {}
  ): Promise<ICopilotCommitDiff> {
    return this.request('copilot_commit_diff_detail', { path }, opts)
  }

  private async repositoryCopilot(
    repositoryPath: string,
    method: 'copilot_commit_diff_detail' | 'copilot_generate_commit_message',
    params: Record<string, unknown>,
    options: BackendRequestOptions
  ): Promise<unknown> {
    if (this.transport === 'wry') {
      return this.wry!.request(
        'copilot',
        { repository_path: repositoryPath, operation: { ...params, method } },
        options
      )
    }
    return this.repositoryRead(repositoryPath, method, params, options)
  }

  async copilotCommitDiffForRepository(
    repositoryPath: string,
    path: BackendGitPath,
    options: BackendRequestOptions = {},
    originalPath?: BackendGitPath
  ): Promise<ICopilotCommitDiff> {
    const result = await this.repositoryCopilot(
      repositoryPath,
      'copilot_commit_diff_detail',
      {
        path,
        ...(originalPath === undefined ? {} : { original_path: originalPath }),
      },
      options
    )
    if (!isCopilotCommitDiff(result)) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid Copilot diff response',
        retryable: false,
      })
    }
    return result
  }

  async copilotGenerateForRepository(
    repositoryPath: string,
    opts: ICopilotCommitOptions & { readonly account: ICopilotAccount },
    options: BackendRequestOptions = {}
  ): Promise<{ summary: string; description: string; basis: string }> {
    const result = await this.repositoryCopilot(
      repositoryPath,
      'copilot_generate_commit_message',
      { ...opts, paths: opts.paths ?? [] },
      copilotDeadline(opts.timeout_ms, options)
    )
    if (
      !isRecord(result) ||
      typeof result.summary !== 'string' ||
      result.summary.trim().length === 0 ||
      typeof result.description !== 'string' ||
      (result.basis !== 'staged_diff' && result.basis !== 'working_tree_diff')
    ) {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid Copilot commit message response',
        retryable: false,
      })
    }
    return {
      summary: result.summary,
      description: result.description,
      basis: result.basis,
    }
  }

  copilotGenerateCommitMessage(
    opts: ICopilotCommitOptions = {},
    backendOpts: BackendRequestOptions = {}
  ): Promise<{ summary: string; description: string; basis: string }> {
    const requestOptions =
      this.transportKind === 'ws'
        ? copilotDeadline(opts.timeout_ms, backendOpts)
        : backendOpts
    return this.request(
      'copilot_generate_commit_message',
      {
        paths: opts.paths ?? [],
        remove_paths: opts.remove_paths,
        system_prompt: opts.system_prompt,
        source: opts.source,
        partial_selections: opts.partial_selections,
        amend_head: opts.amend_head,
        expected_head: opts.expected_head,
        rule_descriptions: opts.rule_descriptions,
        account: opts.account,
        model: opts.model,
        reasoning_effort: opts.reasoning_effort,
        timeout_ms: opts.timeout_ms,
        provider: opts.provider,
      },
      requestOptions
    )
  }
}

// Singleton — shared by all stores; renderer subscribes/proxies once.
let singleton: BackendClient | null = null
export function backend(): BackendClient {
  if (!singleton) singleton = new BackendClient()
  return singleton
}
