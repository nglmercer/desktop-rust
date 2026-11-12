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

export interface ISelectedCommitReview {
  readonly basis: string
  readonly tree: string
  readonly head: string | null
  readonly branch: string | null
  readonly review: {
    readonly threshold_bytes: number
    readonly staged_count: number
    readonly files: ReadonlyArray<BackendGitPath>
    readonly large_files: ReadonlyArray<{ readonly path: BackendGitPath; readonly display_path: string; readonly oid: string; readonly size: number; readonly tracked_by_lfs: boolean }>
  }
}

export type SelectedCommitSelection = Pick<ICopilotCommitOptions, 'paths' | 'remove_paths' | 'partial_selections' | 'amend_head' | 'expected_head'> & { readonly allow_empty?: boolean }
export type CommitCheckpoint = { readonly id: string; readonly state: 'prepared'; readonly head: string | null; readonly branch: string | null } | { readonly id: string; readonly state: 'invalid' }
export interface CommitCheckpointInspection {
  readonly id: string
  readonly state: 'prepared'
  readonly basis: string
  readonly recorded_files_verified: true
  readonly index_coverage_verified: true
  readonly objects_verified: number
  readonly lfs_verified: number
  readonly shared_indexes_verified: number
  readonly head_matches: boolean
  readonly branch_matches: boolean
  readonly index_matches: boolean
  readonly operation_matches: boolean
}

export type BackendReadableGitSetting =
  | 'core.autocrlf'
  | 'core.safecrlf'
  | 'pull.ff'
  | 'pull.rebase'
  | 'trailer.separators'
  | 'commit.gpgsign'

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

export interface IDiscardRecoveryRecord {
  readonly id: string
  readonly state: 'applied' | 'prepared' | 'undone' | 'invalid'
  readonly createdMs: number
  readonly purpose: 'discard' | 'conflict_choice' | 'conflict_stage' | 'unknown'
  readonly files: ReadonlyArray<string>
  readonly unavailableLfs: number
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
        method !== 'git_info' &&
        (opts.signal !== undefined || opts.deadlineMs !== undefined))
    ) {
      throw new BackendError({
        code: 'unsupported',
        message: 'Wry backend request options are not implemented',
        retryable: false,
      })
    }
    const channel = `desktop.backend.${method}`
    if (method === 'read' || method === 'copilot' || method === 'git_info') {
      if (opts.signal && !this.capabilities().cancel) {
        throw new BackendError({
          code: 'unsupported',
          message: 'Cancellation requires the native IPC transport',
          retryable: false,
        })
      }
      const maximum = method === 'copilot' ? 330_000 : 60_000
      const deadline = opts.deadlineMs ?? (method === 'copilot' ? 170_000 : 60_000)
      if (!Number.isInteger(deadline) || deadline < 1 || deadline > maximum) {
        throw new BackendError({
          code: 'invalid_request',
          message: `deadlineMs must be between 1 and ${maximum}`,
          retryable: false,
        })
      }
      const requestId = randomUUID()
      const identityWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        ((params.operation.type === 'set' && params.operation.key !== 'init.defaultbranch') ||
          params.operation.type === 'set_repository' ||
          params.operation.type === 'unset_repository')
      const settingsWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'set' &&
        params.operation.key === 'init.defaultbranch'
      const lfsWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        (params.operation.type === 'lfs_install' || params.operation.type === 'lfs_install_repository')
      const configPathWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'global_config_path'
      const descriptionWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'set_description'
      const creationWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        (params.operation.type === 'write_creation_file' || params.operation.type === 'initial_commit')
      const remoteWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'manage_remote'
      const ignoreWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'git_ignore' &&
        isRecord(params.operation.action) &&
        params.operation.action.type !== 'read'
      const discardWrite =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'reviewed_discard' &&
        isRecord(params.operation.operation) &&
        (params.operation.operation.method === 'discard' || params.operation.operation.method === 'discard_batch' || params.operation.operation.method === 'undo_discard' || params.operation.operation.method === 'forget_discard')
      const initialization =
        method === 'git_info' &&
        isRecord(params) &&
        isRecord(params.operation) &&
        params.operation.type === 'initialize_repository'
      const mutating = identityWrite || settingsWrite || lfsWrite || configPathWrite || descriptionWrite || creationWrite || remoteWrite || ignoreWrite || discardWrite || initialization
      const nativeError = (error: unknown): BackendError => {
        const result = toBackendError(error)
        if (!mutating || !result.retryable) return result
        return new BackendError({
          code: result.code,
          message: result.message,
          retryable: false,
          jobId: result.jobId,
          recoveryId: result.recoveryId,
          details: result.details,
        })
      }
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
                  retryable:
                    method === 'read' ||
                    (method === 'git_info' &&
                      isRecord(params) &&
                      isRecord(params.operation) &&
                      (params.operation.type === 'read' || params.operation.type === 'version' || params.operation.type === 'lfs_status')),
                })
              )
            }
          })
        const abort = () =>
          cancel(
            'cancelled',
            identityWrite
              ? 'Git identity update cancelled; inspect Git settings before retrying'
              : settingsWrite
              ? 'Git setting update cancelled; inspect Git settings before retrying'
              : configPathWrite
              ? 'Git config preparation cancelled; check the configuration file before retrying'
              : descriptionWrite
              ? 'Description update cancelled; inspect the repository description before retrying'
              : creationWrite
              ? 'Repository creation operation cancelled; inspect the repository and index before retrying'
              : remoteWrite
              ? 'Remote update cancelled; inspect Git configuration before retrying'
              : discardWrite
              ? 'Discard operation cancelled; inspect the file and recovery history before retrying'
              : initialization
              ? 'Repository initialization cancelled; inspect the destination before retrying'
              : lfsWrite
              ? 'Git LFS setup cancelled; inspect Git settings and hooks before retrying'
              : 'Backend operation cancelled'
          )
        opts.signal?.addEventListener('abort', abort, { once: true })
        timer = setTimeout(
          () =>
            cancel(
              'ETIMEDOUT',
              identityWrite
                ? 'Git identity update timed out; inspect Git settings before retrying'
                : settingsWrite
                ? 'Git setting update timed out; inspect Git settings before retrying'
                : configPathWrite
                ? 'Git config preparation timed out; check the configuration file before retrying'
                : descriptionWrite
                ? 'Description update timed out; inspect the repository description before retrying'
                : creationWrite
                ? 'Repository creation operation timed out; inspect the repository and index before retrying'
                : remoteWrite
                ? 'Remote update timed out; inspect Git configuration before retrying'
                : discardWrite
                ? 'Discard operation timed out; inspect the file and recovery history before retrying'
                : initialization
                ? 'Repository initialization timed out; inspect the destination before retrying'
                : lfsWrite
                ? 'Git LFS setup timed out; inspect Git settings and hooks before retrying'
                : 'Backend operation timed out'
            ),
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
              error => finish(() => reject(nativeError(error)))
            )
        } catch (error) {
          finish(() => reject(nativeError(error)))
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

  /** Bind the browser bridge to the selected local repository. */
  async selectBrowserRepository(path: string | null): Promise<void> {
    if (this.transport !== 'ws') return
    const result = path === null
      ? await this.request('clear_repository', {})
      : await this.request('open_repository', { path })
    if (path === null) {
      if (result !== null) throw new BackendError({ code: 'invalid_response', message: 'Invalid repository clear response', retryable: false })
    } else if (!isRecord(result) || typeof result.root_id !== 'string' || !/^[a-f0-9]{64}$/.test(result.root_id)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid repository selection response', retryable: false })
    }
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

  /** Read the installed Git version through the Rust host. */
  async gitVersion(): Promise<string> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'version' } })
      : await this.request('git_version', {})
    if (typeof value !== 'string' || value.length === 0 || value.length > 256) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid Git version response', retryable: false })
    }
    return value
  }

  /** Prepare the global Git config file and return the path chosen by Git. */
  async globalGitConfigPath(): Promise<string> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'global_config_path' } })
      : await this.request('git_global_config_path', {})
    if (typeof value !== 'string' || value.length === 0 || value.length > 65536 || value.includes('\0')) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid global Git configuration path', retryable: false })
    }
    return value
  }

  /** Check Git LFS availability without invoking a renderer process shim. */
  async lfsAvailable(): Promise<boolean> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'lfs_status' } })
      : await this.request('lfs_tool_status', {})
    if (!isRecord(value) || typeof value.available !== 'boolean') {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid Git LFS status response', retryable: false })
    }
    return value.available
  }

  /** Install the global LFS filters through the Rust host. */
  async installGlobalLfsFilters(force: boolean): Promise<void> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'lfs_install', force } })
      : await this.request('lfs_install', { scope: 'global', force, skip_repo: true, skip_smudge: false })
    this.validateLfsInstallResult(value)
  }

  /** Install LFS hooks for the requested repository. */
  async installRepositoryLfsHooks(repositoryPath: string, force: boolean): Promise<void> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'lfs_install_repository', repository_path: repositoryPath, force } })
      : await this.request('lfs_repository', { path: repositoryPath, action: { type: 'install_hooks', force } })
    this.validateLfsInstallResult(value)
  }

  private validateLfsInstallResult(value: unknown): void {
    if (!isRecord(value) || !Number.isInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid Git LFS setup response', retryable: false })
    }
    if (value.code !== 0) {
      throw new BackendError({ code: 'lfs_install_failed', message: 'Git LFS configuration was not fully installed; inspect existing hooks and filters before retrying', retryable: false })
    }
  }

  /** Read whether the repository has any Git LFS tracking patterns. */
  async repositoryUsesLfs(repositoryPath: string): Promise<boolean> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'lfs_status', {})
      : await this.request('lfs_repository', { path: repositoryPath, action: { type: 'status' } })
    if (!isRecord(value) || typeof value.available !== 'boolean' ||
        (value.available ? typeof value.using_lfs !== 'boolean' : value.using_lfs !== null)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid repository Git LFS status response', retryable: false })
    }
    return value.using_lfs === true
  }

  /** Read effective Git LFS attributes for literal repository-relative paths. */
  async repositoryLfsFileTracking(repositoryPath: string, paths: ReadonlyArray<string>): Promise<ReadonlyArray<boolean>> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'lfs_file_tracking', { paths })
      : await this.request('lfs_repository', { path: repositoryPath, action: { type: 'file_tracking', paths } })
    if (!Array.isArray(value) || value.length !== paths.length || !value.every(item => typeof item === 'boolean')) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid Git LFS attribute response', retryable: false })
    }
    return value
  }

  /** Read an allowlisted local or inherited repository setting. */
  async repositoryGitReadSetting(
    repositoryPath: string,
    key: BackendReadableGitSetting,
    onlyLocal: boolean,
    canonicalBool: boolean
  ): Promise<string | null> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'git_read_setting', { key, only_local: onlyLocal, canonical_bool: canonicalBool })
      : await this.request('git_read_repository_setting', { path: repositoryPath, key, only_local: onlyLocal, canonical_bool: canonicalBool })
    if (value !== null && (typeof value !== 'string' || (canonicalBool && value !== 'true' && value !== 'false'))) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid Git setting response', retryable: false })
    }
    return value
  }

  /** Read the selected repository's Git description. */
  async repositoryGitDescription(repositoryPath: string): Promise<string> {
    const value = await this.repositoryRead(repositoryPath, 'git_description', {})
    if (typeof value !== 'string') {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid repository description response', retryable: false })
    }
    return value
  }

  /** Return a credential-free link for a locally configured submodule. */
  async repositorySubmoduleDisplayUrl(repositoryPath: string, path: string): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, 'submodule_display_url', this.transport === 'wry' ? { submodule_path: path } : { path })
    if (value === null) return null
    if (typeof value === 'string' && value.length <= 16384) {
      try {
        const url = new URL(value)
        if (url.protocol === 'https:' && url.username === '' && url.password === '' && url.search === '' && url.hash === '') return value
      } catch {}
    }
    throw new BackendError({ code: 'invalid_response', message: 'Invalid submodule link response', retryable: false })
  }

  /** Write the description of the explicitly requested repository. */
  async setRepositoryGitDescription(repositoryPath: string, description: string): Promise<void> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'set_description', repository_path: repositoryPath, description } })
      : await this.request('set_repository_description', { path: repositoryPath, description })
    if (value !== null) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid repository description update response', retryable: false })
    }
  }

  /** Read or update the repository-root ignore file. */
  async repositoryGitIgnore(repositoryPath: string, action: { readonly type: 'read' } | { readonly type: 'save' | 'append'; readonly text: string }): Promise<string | null> {
    const params = { path: repositoryPath, action }
    if (this.transport !== 'wry' && new TextEncoder().encode(JSON.stringify({ ...params, method: 'repository_git_ignore', id: Number.MAX_SAFE_INTEGER })).length > 65536) {
      throw new BackendError({ code: 'invalid_request', message: 'Ignore file update exceeds the bridge request limit', retryable: false })
    }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'git_ignore', repository_path: repositoryPath, action } })
      : await this.request('repository_git_ignore', params)
    if (value !== null && (action.type !== 'read' || typeof value !== 'string')) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid ignore file response', retryable: false })
    }
    return value as string | null
  }

  /** Write a file offered by the new-repository dialog. */
  async writeRepositoryCreationFile(repositoryPath: string, kind: 'readme' | 'gitignore' | 'license' | 'git_attributes', contents: string): Promise<void> {
    const params = { path: repositoryPath, kind, contents }
    if (this.transport !== 'wry' && new TextEncoder().encode(JSON.stringify({ ...params, method: 'write_repository_creation_file', id: Number.MAX_SAFE_INTEGER })).length > 65536) {
      throw new BackendError({ code: 'invalid_request', message: 'Creation file exceeds the bridge request limit', retryable: false })
    }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'write_creation_file', repository_path: repositoryPath, kind, contents } })
      : await this.request('write_repository_creation_file', params)
    if (value !== null) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid creation file update response', retryable: false })
    }
  }

  /** Stage the displayed paths and make the first commit at an explicit root. */
  async createInitialCommit(repositoryPath: string, paths: ReadonlyArray<string>): Promise<void> {
    const params = { path: repositoryPath, paths }
    if (this.transport !== 'wry' && new TextEncoder().encode(JSON.stringify({ ...params, method: 'create_initial_commit', id: Number.MAX_SAFE_INTEGER })).length > 65536) {
      throw new BackendError({ code: 'invalid_request', message: 'Initial commit paths exceed the bridge request limit', retryable: false })
    }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'initial_commit', repository_path: repositoryPath, paths } })
      : await this.request('create_initial_commit', params)
    if (!isRecord(value) || typeof value.code !== 'number' || typeof value.stderr !== 'string') {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid initial commit response; inspect the repository before retrying', retryable: false })
    }
    if (value.code !== 0) {
      throw new BackendError({ code: 'initial_commit_failed', message: `${value.stderr || 'Initial commit failed'}; inspect staged files before retrying`, retryable: false })
    }
  }

  /** Initialize the requested directory without changing the selected repository. */
  async initializeDesktopRepository(path: string, defaultBranch: string): Promise<void> {
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'initialize_repository', repository_path: path, default_branch: defaultBranch } })
      : await this.request('initialize_desktop_repository', { path, default_branch: defaultBranch })
    if (value !== null) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid repository initialization response', retryable: false })
    }
  }

  /** Resolve the author or committer identity Git would use. */
  async repositoryEffectiveGitIdentity(repositoryPath: string, role: 'author' | 'committer'): Promise<string | null> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, role === 'author' ? 'git_author_identity' : 'git_committer_identity', {})
      : await this.request('git_repository_effective_identity', { path: repositoryPath, role })
    if (value !== null && (typeof value !== 'string' || value.length > 65536)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid effective Git identity response', retryable: false })
    }
    return value
  }

  /** Resolve ordinary fetch remotes, or return null for sensitive URL forms. */
  async repositorySafeFetchRemotes(repositoryPath: string): Promise<ReadonlyArray<{ readonly name: string; readonly url: string }> | null> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'safe_fetch_remotes', {})
      : await this.request('repository_safe_fetch_remotes', { path: repositoryPath })
    if (value === null) return null
    if (!Array.isArray(value) || value.length > 1000 || !value.every(remote =>
      isRecord(remote) && typeof remote.name === 'string' && remote.name.length > 0 && typeof remote.url === 'string' && remote.url.length > 0
    )) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid remote discovery response', retryable: false })
    }
    return value as Array<{ name: string; url: string }>
  }

  async repositoryWorktrees(repositoryPath: string, fromGitDir = false): Promise<ReadonlyArray<{ readonly path: string; readonly head: string; readonly branch: string | null; readonly isDetached: boolean; readonly type: 'main' | 'linked'; readonly isLocked: boolean; readonly isPrunable: boolean }>> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, fromGitDir ? 'worktrees_from_git_dir' : 'worktrees', {})
      : await this.request(fromGitDir ? 'repository_worktrees_from_git_dir' : 'repository_worktrees', { path: repositoryPath })
    const invalid = () => new BackendError({ code: 'invalid_response', message: 'Invalid working tree list', retryable: false })
    if (!Array.isArray(value)) throw invalid()
    const seen = new Set<string>()
    return value.map((entry, index) => {
      if (!isRecord(entry) || !isBackendGitPath(entry.path) || typeof entry.head !== 'string' || !(entry.branch === null || typeof entry.branch === 'string') || typeof entry.detached !== 'boolean' || !(entry.locked === null || typeof entry.locked === 'string') || !(entry.prunable === null || typeof entry.prunable === 'string') || entry.main !== (index === 0) || typeof entry.bare !== 'boolean') throw invalid()
      let path: string
      try {
        path = typeof entry.path === 'string' ? entry.path : new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(new Uint8Array(entry.path.bytes))
      } catch {
        throw new BackendError({ code: 'unsupported_path', message: 'A working tree path cannot be represented safely', retryable: false })
      }
      if (!path || path.includes('\0') || seen.has(path)) throw invalid()
      seen.add(path)
      return { path, head: entry.head, branch: entry.branch, isDetached: entry.detached, type: entry.main ? 'main' : 'linked', isLocked: entry.locked !== null, isPrunable: entry.prunable !== null }
    })
  }

  /** Resolve one remote, retaining a distinct fallback for sensitive URLs. */
  async repositorySafeRemoteUrl(repositoryPath: string, name: string): Promise<{ readonly kind: 'resolved'; readonly url: string } | { readonly kind: 'missing' | 'unsupported' }> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'safe_remote_url', { remote_name: name })
      : await this.request('repository_safe_remote_url', { path: repositoryPath, name })
    if (isRecord(value) && (value.kind === 'missing' || value.kind === 'unsupported')) return { kind: value.kind }
    if (isRecord(value) && value.kind === 'resolved' && typeof value.url === 'string' && value.url.length > 0 && value.url.length <= 16384) return { kind: 'resolved', url: value.url }
    throw new BackendError({ code: 'invalid_response', message: 'Invalid remote URL response', retryable: false })
  }

  /** Apply a supported remote configuration change without echoing its URL. */
  async manageRepositoryRemote(repositoryPath: string, action: { readonly type: 'add' | 'set_url'; readonly name: string; readonly url: string } | { readonly type: 'remove'; readonly name: string }): Promise<'updated' | 'unsupported'> {
    const params = { path: repositoryPath, action }
    if (this.transport !== 'wry' && new TextEncoder().encode(JSON.stringify({ ...params, method: 'manage_repository_remote', id: Number.MAX_SAFE_INTEGER })).length > 65536) {
      throw new BackendError({ code: 'invalid_request', message: 'Remote update exceeds the bridge request limit', retryable: false })
    }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'manage_remote', repository_path: repositoryPath, action } })
      : await this.request('manage_repository_remote', params)
    if (!isRecord(value) || (value.kind !== 'updated' && value.kind !== 'unsupported')) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid remote update response; inspect Git configuration before retrying', retryable: false })
    }
    return value.kind
  }

  /** Read a symbolic ref in the requested repository. */
  async repositorySymbolicRef(repositoryPath: string, reference: string): Promise<string | null> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'symbolic_ref', { reference })
      : await this.request('repository_symbolic_ref', { path: repositoryPath, reference })
    if (value !== null && (typeof value !== 'string' || value.length > 65536)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid symbolic ref response', retryable: false })
    }
    return value
  }

  /** Read a repository's local or inherited Git identity through Rust. */
  async repositoryGitIdentityValue(
    repositoryPath: string,
    key: 'user.name' | 'user.email',
    onlyLocal: boolean
  ): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, 'git_identity_value', {
      key,
      only_local: onlyLocal,
    })
    if (value !== null && typeof value !== 'string') {
      throw new BackendError({
        code: 'invalid_response',
        message: 'Invalid repository Git identity response',
        retryable: false,
      })
    }
    return value
  }

  /** Update or remove one local identity field for the intended repository. */
  async setRepositoryGitIdentityValue(
    repositoryPath: string,
    key: 'user.name' | 'user.email',
    value: string | null
  ): Promise<void> {
    if (this.transport === 'wry') {
      const operation = value === null
        ? { type: 'unset_repository', repository_path: repositoryPath, key }
        : { type: 'set_repository', repository_path: repositoryPath, key, value }
      await this.wry!.request('git_info', { operation }, {})
      return
    }
    await this.repositoryRead(repositoryPath, 'set_git_setting', {
      scope: 'local', key, value,
    })
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

  /** Read staged paths without losing Git's filename boundaries. */
  async repositoryIndexChanges(repositoryPath: string): Promise<ReadonlyArray<{ readonly path: string; readonly status: 'A' | 'D' | 'M' | 'T' | 'U' | 'X' }>> {
    const value = await this.repositoryRead(repositoryPath, 'index_changes', {})
    if (!Array.isArray(value)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid index changes response', retryable: false })
    }
    const seen = new Set<string>()
    return value.map(entry => {
      if (!isRecord(entry) || !isBackendGitPath(entry.path) || typeof entry.status !== 'string' || !['A', 'D', 'M', 'T', 'U', 'X'].includes(entry.status)) {
        throw new BackendError({ code: 'invalid_response', message: 'Invalid index changes response', retryable: false })
      }
      let path: string
      try {
        path = typeof entry.path === 'string' ? entry.path : new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(new Uint8Array(entry.path.bytes))
      } catch {
        throw new BackendError({ code: 'unsupported_path', message: 'A staged filename cannot be represented safely; inspect the index before discarding changes', retryable: false })
      }
      if (path.length === 0 || path.includes('\0') || seen.has(path)) {
        throw new BackendError({ code: 'invalid_response', message: 'Invalid or duplicate staged path', retryable: false })
      }
      seen.add(path)
      return { path, status: entry.status as 'A' | 'D' | 'M' | 'T' | 'U' | 'X' }
    })
  }

  async repositoryRendererCommit(repositoryPath: string, revision: string): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, 'renderer_commit', this.transport === 'wry' ? { commitish: revision } : { revision })
    if (value !== null && (typeof value !== 'string' || value.length > 3_000_000 || value.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(value))) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid commit metadata response', retryable: false })
    }
    return value as string | null
  }

  async repositoryRendererHistory(repositoryPath: string, revision: string | undefined, limit: number | undefined, skip: number | undefined, firstParent: boolean, excludeRemotes: boolean): Promise<string | null> {
    if ([limit, skip].some(value => value !== undefined && (!Number.isSafeInteger(value) || value < 0))) throw new BackendError({ code: 'invalid_request', message: 'Invalid history pagination', retryable: false })
    const params = { ...(this.transport === 'wry' ? { commitish: revision } : { revision }), limit, skip, first_parent: firstParent, exclude_remotes: excludeRemotes }
    const value = await this.repositoryRead(repositoryPath, 'renderer_history', params)
    if (value !== null && (typeof value !== 'string' || value.length > 3_000_000 || value.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(value))) throw new BackendError({ code: 'invalid_response', message: 'Invalid history response', retryable: false })
    return value as string | null
  }

  async repositoryRendererReflog(repositoryPath: string, kind: 'branches' | 'stash'): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, kind === 'branches' ? 'renderer_recent_branches' : 'renderer_stash_log', {})
    if (value !== null && (typeof value !== 'string' || value.length > 2 * 1024 * 1024)) throw new BackendError({ code: 'invalid_response', message: 'Invalid reflog response', retryable: false })
    return value as string | null
  }

  async repositoryProtectedResources(repositoryPath: string): Promise<{ readonly paths: ReadonlyArray<string>; readonly objectDirectories: ReadonlyArray<string>; readonly objectRoutingDiagnostics: string }> {
    const value = this.transport === 'wry'
      ? await this.repositoryRead(repositoryPath, 'protected_resources', {})
      : await this.request('repository_protected_resources', { path: repositoryPath })
    if (!isRecord(value) || !Array.isArray(value.paths) || !Array.isArray(value.object_directories) || typeof value.object_routing_diagnostics !== 'string' || value.paths.length < 4 || value.paths.length > 32768 || value.object_directories.length < 1 || value.object_directories.length > 16384 || ![...value.paths, ...value.object_directories].every(path => typeof path === 'string' && path.length > 0 && !path.includes('\0'))) throw new BackendError({ code: 'invalid_response', message: 'Invalid repository resource paths', retryable: false })
    const paths = value.paths as string[]
    const objectDirectories = value.object_directories as string[]
    if (paths.length !== 2 * (3 + objectDirectories.length) || paths[0] !== repositoryPath || !objectDirectories.every((path, index) => paths[3 + index] === path)) throw new BackendError({ code: 'invalid_response', message: 'Incomplete repository resource paths', retryable: false })
    return { paths, objectDirectories, objectRoutingDiagnostics: value.object_routing_diagnostics }
  }

  async repositoryBranchCheckouts(repositoryPath: string, afterDate: Date): Promise<string | null> {
    const value = await this.repositoryRead(repositoryPath, 'renderer_branch_checkouts', { after_date: afterDate.toISOString() })
    if (value !== null && (typeof value !== 'string' || value.length > 2 * 1024 * 1024)) throw new BackendError({ code: 'invalid_response', message: 'Invalid checkout history response', retryable: false })
    return value as string | null
  }

  async repositoryRendererTagRefs(repositoryPath: string): Promise<string> {
    const value = await this.repositoryRead(repositoryPath, 'renderer_tag_refs', {})
    if (typeof value !== 'string' || value.length > 2 * 1024 * 1024) throw new BackendError({ code: 'invalid_response', message: 'Invalid tag refs response', retryable: false })
    return value
  }

  async repositoryLargeFiles(repositoryPath: string, paths: ReadonlyArray<string>): Promise<ReadonlyArray<boolean>> {
    if (paths.length > 1000 || new TextEncoder().encode(JSON.stringify({ paths })).length > 60_000) throw new BackendError({ code: 'invalid_request', message: 'File-size review exceeds the request limit', retryable: false })
    const value = await this.repositoryRead(repositoryPath, 'renderer_large_files', { paths })
    if (!Array.isArray(value) || value.length !== paths.length || !value.every(entry => typeof entry === 'boolean')) throw new BackendError({ code: 'invalid_response', message: 'Invalid file-size review response', retryable: false })
    return value
  }

  async repositoryRendererCommitChanges(repositoryPath: string, revision: string): Promise<string> {
    const value = await this.repositoryRead(repositoryPath, 'renderer_commit_changes', this.transport === 'wry' ? { commitish: revision } : { revision })
    if (typeof value !== 'string' || value.length > 2 * 1024 * 1024) throw new BackendError({ code: 'invalid_response', message: 'Invalid changed files response', retryable: false })
    return value
  }

  private async repositoryRendererRequest(repositoryPath: string, method: 'renderer_commit_diff' | 'renderer_blob_prefix' | 'renderer_working_diff' | 'diff_detail' | 'select_diff' | 'select_renamed_diff' | 'discard_preview' | 'discard' | 'undo_discard' | 'list_discards' | 'forget_discard' | 'discard_batch_preview' | 'discard_batch', params: Record<string, unknown>): Promise<unknown> {
    if (this.transport === 'wry') {
      return this.request('git_info', { operation: { type: 'reviewed_discard', repository_path: repositoryPath, operation: { method, ...params } } })
    }
    return this.repositoryRead(repositoryPath, method, params)
  }

  async repositoryRendererCommitDiff(repositoryPath: string, revision: string, path: string, oldPath: string | undefined, hideWhitespace: boolean): Promise<string> {
    const value = await this.repositoryRendererRequest(repositoryPath, 'renderer_commit_diff', { revision, path, old_path: oldPath ?? null, hide_whitespace: hideWhitespace })
    if (typeof value !== 'string' || value.length > 3_000_000 || value.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(value)) throw new BackendError({ code: 'invalid_response', message: 'Invalid commit diff response', retryable: false })
    return value
  }

  /** Capture a review basis without publishing the selected index or committing. */
  async repositorySelectedCommitPreview(repositoryPath: string, selection: SelectedCommitSelection): Promise<ISelectedCommitReview> {
    const operation = { method: 'selected_commit_preview', paths: selection.paths ?? [], remove_paths: selection.remove_paths ?? [], partial_selections: selection.partial_selections ?? [], allow_empty: selection.allow_empty ?? false, amend_head: selection.amend_head ?? null, expected_head: selection.expected_head ?? null }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Selected commit review exceeds the request limit', retryable: false })
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'selected_commit_preview', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (!isRecord(value) || typeof value.basis !== 'string' || !/^[a-f0-9]{64}$/.test(value.basis) || typeof value.tree !== 'string' || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(value.tree) || !(value.head === null || (typeof value.head === 'string' && /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(value.head))) || !(value.branch === null || (typeof value.branch === 'string' && value.branch.length <= 4096 && !value.branch.includes('\0')))) throw new BackendError({ code: 'invalid_response', message: 'Invalid selected commit review response', retryable: false })
    const review = value.review
    const invalid = () => new BackendError({ code: 'invalid_response', message: 'Invalid selected index review', retryable: false })
    if (!isRecord(review) || typeof review.threshold_bytes !== 'number' || !Number.isSafeInteger(review.threshold_bytes) || review.threshold_bytes <= 0 || !Array.isArray(review.files) || review.files.length > 32768 || !review.files.every(isBackendGitPath) || review.staged_count !== review.files.length || !Array.isArray(review.large_files) || review.large_files.length > 1000) throw invalid()
    const threshold = review.threshold_bytes
    const pathKey = (path: BackendGitPath): string => {
      const bytes = typeof path === 'string' ? [...new TextEncoder().encode(path)] : path.bytes
      if (bytes.length === 0 || bytes.includes(0)) throw invalid()
      return bytes.join(',')
    }
    const paths = new Set(review.files.map(pathKey))
    if (paths.size !== review.files.length) throw invalid()
    const largePaths = new Set<string>()
    for (const file of review.large_files) {
      if (!isRecord(file) || !isBackendGitPath(file.path) || typeof file.display_path !== 'string' || typeof file.oid !== 'string' || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(file.oid) || typeof file.size !== 'number' || !Number.isSafeInteger(file.size) || file.size <= threshold || typeof file.tracked_by_lfs !== 'boolean') throw invalid()
      const key = pathKey(file.path)
      if (!paths.has(key) || largePaths.has(key)) throw invalid()
      largePaths.add(key)
    }
    return value as unknown as ISelectedCommitReview
  }

  async verifySelectedIndex(repositoryPath: string, review: Pick<ISelectedCommitReview, 'tree' | 'head' | 'branch'>): Promise<boolean> {
    const operation = { method: 'verify_selected_index', tree: review.tree, head: review.head, branch: review.branch }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'selected_commit_preview', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (!isRecord(value) || typeof value.matches !== 'boolean') throw new BackendError({ code: 'invalid_response', message: 'Invalid staged tree verification', retryable: false })
    return value.matches
  }

  private async commitCheckpointRequest(repositoryPath: string, operation: Record<string, unknown>): Promise<unknown> {
    return this.transport === 'wry'
      ? this.request('git_info', { operation: { type: 'commit_checkpoint', repository_path: repositoryPath, operation } })
      : this.repositoryRead(repositoryPath, operation.method as string, operation)
  }

  async prepareSelectedCommit(repositoryPath: string, selection: SelectedCommitSelection, basis: string): Promise<string> {
    if (!/^[a-f0-9]{64}$/.test(basis)) throw new BackendError({ code: 'invalid_request', message: 'Invalid commit review basis', retryable: false })
    const operation = { method: 'prepare_selected_commit', paths: selection.paths ?? [], remove_paths: selection.remove_paths ?? [], partial_selections: selection.partial_selections ?? [], allow_empty: selection.allow_empty ?? false, amend_head: selection.amend_head ?? null, expected_head: selection.expected_head ?? null, basis }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Selected commit preparation exceeds the transport limit', retryable: false })
    let value: unknown
    try { value = await this.commitCheckpointRequest(repositoryPath, operation) }
    catch (error) { const cause = toBackendError(error); throw new BackendError({ code: cause.code, message: `${cause.message}; inspect commit checkpoints before retrying`, retryable: false }) }
    if (!isRecord(value) || value.state !== 'prepared' || value.basis !== basis || typeof value.id !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value.id)) throw new BackendError({ code: 'invalid_response', message: 'Preparation outcome is uncertain; inspect commit checkpoints', retryable: false })
    return value.id
  }

  async listCommitCheckpoints(repositoryPath: string): Promise<ReadonlyArray<CommitCheckpoint>> {
    const value = await this.commitCheckpointRequest(repositoryPath, { method: 'list_commit_checkpoints' })
    if (!isRecord(value) || !Array.isArray(value.checkpoints) || value.checkpoints.length > 1000) throw new BackendError({ code: 'invalid_response', message: 'Invalid commit checkpoint history', retryable: false })
    const ids = new Set<string>()
    for (const item of value.checkpoints) {
      if (!isRecord(item) || typeof item.id !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(item.id) || (item.state !== 'invalid' && (item.state !== 'prepared' || !(item.head === null || (typeof item.head === 'string' && /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(item.head))) || !(item.branch === null || (typeof item.branch === 'string' && item.branch.length <= 4096 && !item.branch.includes('\0')))))) throw new BackendError({ code: 'invalid_response', message: 'Invalid commit checkpoint history', retryable: false })
      if (ids.has(item.id)) throw new BackendError({ code: 'invalid_response', message: 'Duplicate commit checkpoint ID', retryable: false })
      ids.add(item.id)
    }
    return value.checkpoints as CommitCheckpoint[]
  }

  async forgetCommitCheckpoint(repositoryPath: string, id: string): Promise<void> {
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw new BackendError({ code: 'invalid_request', message: 'Invalid commit checkpoint ID', retryable: false })
    let value: unknown
    try { value = await this.commitCheckpointRequest(repositoryPath, { method: 'forget_commit_checkpoint', checkpoint_id: id }) }
    catch (error) { const cause = toBackendError(error); throw new BackendError({ code: cause.code, message: `${cause.message}; inspect commit checkpoints before retrying`, retryable: false }) }
    if (!isRecord(value) || value.id !== id || value.forgotten !== true) throw new BackendError({ code: 'invalid_response', message: 'Checkpoint deletion outcome is uncertain; inspect commit checkpoints', retryable: false })
  }

  async inspectCommitCheckpoint(repositoryPath: string, id: string): Promise<CommitCheckpointInspection> {
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw new BackendError({ code: 'invalid_request', message: 'Invalid commit checkpoint ID', retryable: false })
    const value = await this.commitCheckpointRequest(repositoryPath, { method: 'inspect_commit_checkpoint', checkpoint_id: id })
    if (!isRecord(value) || value.id !== id || value.state !== 'prepared' || typeof value.basis !== 'string' || !/^[a-f0-9]{64}$/.test(value.basis) || value.recorded_files_verified !== true || value.index_coverage_verified !== true || ![value.objects_verified, value.lfs_verified, value.shared_indexes_verified].every(count => typeof count === 'number' && Number.isSafeInteger(count) && count >= 0 && count <= 65536) || ![value.head_matches, value.branch_matches, value.index_matches, value.operation_matches].every(flag => typeof flag === 'boolean')) throw new BackendError({ code: 'invalid_response', message: 'Invalid commit checkpoint inspection', retryable: false })
    return value as unknown as CommitCheckpointInspection
  }

  async revalidateCommitCheckpoint(repositoryPath: string, id: string): Promise<{ readonly basis: string; readonly matches: boolean }> {
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw new BackendError({ code: 'invalid_request', message: 'Invalid commit checkpoint ID', retryable: false })
    const value = await this.commitCheckpointRequest(repositoryPath, { method: 'revalidate_commit_checkpoint', checkpoint_id: id })
    if (!isRecord(value) || value.id !== id || typeof value.basis !== 'string' || !/^[a-f0-9]{64}$/.test(value.basis) || typeof value.matches !== 'boolean') throw new BackendError({ code: 'invalid_response', message: 'Invalid commit checkpoint revalidation', retryable: false })
    return { basis: value.basis, matches: value.matches }
  }

  async repositoryInterpretTrailers(repositoryPath: string, message: string, trailers: ReadonlyArray<{ readonly token: string; readonly value: string }>, parse: boolean, unfold: boolean): Promise<string> {
    const operation = { method: 'interpret_trailers', message, trailers, parse, unfold }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Commit trailer request exceeds the transport limit', retryable: false })
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'interpret_trailers', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (typeof value !== 'string' || value.length > 2 * 1024 * 1024) throw new BackendError({ code: 'invalid_response', message: 'Invalid Git trailer response', retryable: false })
    return value
  }

  async repositoryCheckoutIndex(repositoryPath: string, paths: ReadonlyArray<string>): Promise<void> {
    return this.repositoryMutateIndex(repositoryPath, paths, 'checkout')
  }

  async repositoryResetIndex(repositoryPath: string, paths: ReadonlyArray<string>): Promise<void> {
    return this.repositoryMutateIndex(repositoryPath, paths, 'reset')
  }

  async repositoryUpdateIndex(repositoryPath: string, paths: ReadonlyArray<string>, forceRemove: boolean): Promise<void> {
    if (paths.length === 0 || paths.length > 4096) throw new BackendError({ code: 'invalid_request', message: 'Invalid index selection', retryable: false })
    const operation = { method: 'renderer_update_index', paths, force_remove: forceRemove }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Index selection exceeds the request limit', retryable: false })
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'update_index', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the index before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Index update outcome is uncertain; inspect the index', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Unable to update the index'}; inspect the index`, retryable: false })
  }

  async repositoryCheckoutHeadPaths(repositoryPath: string, paths: ReadonlyArray<string>): Promise<void> {
    return this.repositoryMutateIndex(repositoryPath, paths, 'head')
  }

  async repositoryUnstageAll(repositoryPath: string): Promise<void> {
    return this.repositoryClearIndex(repositoryPath, false)
  }

  async repositoryRemoveIndexAll(repositoryPath: string): Promise<void> {
    return this.repositoryClearIndex(repositoryPath, true)
  }

  async repositoryDeleteInitialHead(repositoryPath: string, expectedOid: string): Promise<void> {
    if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(expectedOid)) throw new BackendError({ code: 'invalid_request', message: 'Invalid expected commit ID', retryable: false })
    const operation = { method: 'renderer_delete_initial_head', expected_oid: expectedOid }
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'delete_initial_head', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect HEAD and the index before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'HEAD deletion outcome is uncertain; inspect HEAD and the index', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Unable to delete HEAD'}; inspect HEAD and the index`, retryable: false })
  }

  private async repositoryClearIndex(repositoryPath: string, remove: boolean): Promise<void> {
    const operation = { method: remove ? 'renderer_remove_index_all' : 'renderer_unstage_all' }
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'reset_index', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the index before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Unstage outcome is uncertain; inspect the index', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Unable to unstage files'}; inspect the index`, retryable: false })
  }

  private async repositoryMutateIndex(repositoryPath: string, paths: ReadonlyArray<string>, action: 'checkout' | 'reset' | 'head'): Promise<void> {
    if ((paths.length === 0 && action !== 'head') || paths.length > 4096) throw new BackendError({ code: 'invalid_request', message: 'Invalid index selection', retryable: false })
    const operation = { method: action === 'checkout' ? 'renderer_checkout_index' : action === 'head' ? 'renderer_checkout_head_paths' : 'renderer_reset_index', paths }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Index selection exceeds the request limit', retryable: false })
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: action === 'reset' ? 'reset_index' : 'checkout_index', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the selected files before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Index operation outcome is uncertain; inspect the selected files', retryable: false })
    if (value.code !== 0 && (action !== 'checkout' || value.code !== 1)) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Index operation failed'}; inspect the selected files`, retryable: false })
  }

  async repositorySubmoduleStatus(repositoryPath: string): Promise<string> {
    const operation = { method: 'renderer_submodule_status' }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'submodule_status', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (typeof value !== 'string' || value.length > 2 * 1024 * 1024) throw new BackendError({ code: 'invalid_response', message: 'Invalid submodule status response', retryable: false })
    return value
  }

  async repositoryResetSubmodules(repositoryPath: string, paths: ReadonlyArray<string>): Promise<void> {
    if (paths.length === 0 || paths.length > 4096 || new Set(paths).size !== paths.length) throw new BackendError({ code: 'invalid_request', message: 'Select distinct submodule paths', retryable: false })
    const operation = { method: 'renderer_reset_submodules', paths }
    const size = new TextEncoder().encode(JSON.stringify({ ...operation, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Submodule selection exceeds the request limit', retryable: false })
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'reset_submodules', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the selected submodules before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Submodule reset outcome is uncertain; inspect the selected submodules', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Submodule reset failed'}; inspect the selected submodules`, retryable: false })
  }

  async repositoryUpdateSubmodules(repositoryPath: string, allowFileProtocol: boolean): Promise<void> {
    const operation = { method: 'renderer_update_submodules', allow_file_protocol: allowFileProtocol }
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'update_submodules', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect submodules before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Submodule update outcome is uncertain; inspect submodules', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: `${value.stderr || 'Submodule update failed'}; inspect submodules`, retryable: false })
  }

  async repositoryMergeBase(repositoryPath: string, first: string, second: string): Promise<string | null> {
    if ([first, second].some(revision => revision.length === 0 || revision.length > 4096 || revision.includes('\0'))) throw new BackendError({ code: 'invalid_request', message: 'Invalid merge-base revision', retryable: false })
    const operation = { method: 'merge_base', first, second }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'merge_base', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (value !== null && (typeof value !== 'string' || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i.test(value))) throw new BackendError({ code: 'invalid_response', message: 'Invalid merge-base result', retryable: false })
    return value as string | null
  }

  async repositoryMergeTreePreview(repositoryPath: string, ours: string, theirs: string): Promise<{ readonly kind: 'clean' | 'invalid' } | { readonly kind: 'conflicts'; readonly conflictedFiles: number }> {
    const oid = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i
    if (!oid.test(ours) || !oid.test(theirs) || ours.length !== theirs.length) throw new BackendError({ code: 'invalid_request', message: 'Invalid merge commit IDs', retryable: false })
    const operation = { method: 'merge_tree_preview', ours, theirs }
    const value = this.transport === 'wry'
      ? await this.request('git_info', { operation: { type: 'merge_tree_preview', repository_path: repositoryPath, operation } })
      : await this.repositoryRead(repositoryPath, operation.method, operation)
    if (isRecord(value) && (value.kind === 'clean' || value.kind === 'invalid')) return { kind: value.kind }
    if (isRecord(value) && value.kind === 'conflicts' && typeof value.conflicted_files === 'number' && Number.isSafeInteger(value.conflicted_files) && value.conflicted_files > 0 && value.conflicted_files <= 1_000_000) return { kind: 'conflicts', conflictedFiles: value.conflicted_files }
    throw new BackendError({ code: 'invalid_response', message: 'Invalid merge preview response', retryable: false })
  }

  async mutateRepositoryTag(repositoryPath: string, operation: { readonly method: 'tag_create_annotated'; readonly name: string; readonly target: string } | { readonly method: 'tag_delete'; readonly name: string }): Promise<void> {
    if (operation.name.length === 0 || operation.name.length > 4096 || operation.name.includes('\0') || (operation.method === 'tag_create_annotated' && (operation.target.length === 0 || operation.target.length > 4096 || operation.target.includes('\0')))) throw new BackendError({ code: 'invalid_request', message: 'Invalid tag request', retryable: false })
    let value: unknown
    try {
      value = this.transport === 'wry'
        ? await this.request('git_info', { operation: { type: 'tag_mutation', repository_path: repositoryPath, operation } })
        : await this.repositoryRead(repositoryPath, operation.method, operation)
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect tags before retrying`, retryable: false })
    }
    if (!isRecord(value) || !Number.isSafeInteger(value.code) || typeof value.stdout !== 'string' || typeof value.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Tag operation outcome is uncertain; inspect tags before retrying', retryable: false })
    if (value.code !== 0) throw new BackendError({ code: 'git_error', message: value.stderr || 'Git tag operation failed', retryable: false })
  }

  async repositoryBlobPrefix(repositoryPath: string, revision: string, path: string, length: number): Promise<string | null> {
    if (!Number.isSafeInteger(length) || length < 0 || length > 1024 * 1024) throw new BackendError({ code: 'invalid_request', message: 'Invalid blob preview length', retryable: false })
    const value = await this.repositoryRendererRequest(repositoryPath, 'renderer_blob_prefix', { revision, path, length })
    if (value !== null && (typeof value !== 'string' || value.length > Math.ceil(length / 3) * 4 || value.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(value))) throw new BackendError({ code: 'invalid_response', message: 'Invalid blob preview response', retryable: false })
    if (typeof value === 'string' && value.length / 4 * 3 - (value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0) > length) throw new BackendError({ code: 'invalid_response', message: 'Oversized blob preview response', retryable: false })
    return value as string | null
  }

  async repositoryRendererWorkingDiff(repositoryPath: string, path: string, base: 'head' | 'index' | 'empty', hideWhitespace: boolean, noTextconv: boolean): Promise<{ stdout: string; stderr: string }> {
    const value = await this.repositoryRendererRequest(repositoryPath, 'renderer_working_diff', { path, base, hide_whitespace: hideWhitespace, no_textconv: noTextconv })
    const valid = (v: unknown): v is string => typeof v === 'string' && v.length <= 3_000_000 && v.length % 4 === 0 && /^[A-Za-z0-9+/]*={0,2}$/.test(v)
    if (!isRecord(value) || !valid(value.stdout) || !valid(value.stderr)) throw new BackendError({ code: 'invalid_response', message: 'Invalid working diff response', retryable: false })
    return { stdout: value.stdout, stderr: value.stderr }
  }

  async repositoryDiscardBatchPreview(repositoryPath: string, paths: ReadonlyArray<string>): Promise<{ readonly basis: string; readonly files: ReadonlyArray<string> }> {
    if (paths.length === 0 || paths.length > 4096 || new Set(paths).size !== paths.length) throw new BackendError({ code: 'invalid_request', message: 'Select distinct files to discard', retryable: false })
    if (this.transport === 'ws' && new TextEncoder().encode(JSON.stringify({ method: 'discard_batch', paths, basis: 'f'.repeat(64), id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length > 65536) throw new BackendError({ code: 'invalid_request', message: 'The selected files exceed the bridge request limit', retryable: false })
    const value = await this.repositoryRendererRequest(repositoryPath, 'discard_batch_preview', { paths })
    if (!isRecord(value) || typeof value.basis !== 'string' || !/^[a-f0-9]{64}$/.test(value.basis) || !Array.isArray(value.files) || value.files.length === 0 || value.files.length > 8192) throw new BackendError({ code: 'invalid_response', message: 'Invalid discard review', retryable: false })
    const seen = new Set<string>()
    const files = value.files.map(entry => {
      if (!isRecord(entry) || !isBackendGitPath(entry.path)) throw new BackendError({ code: 'invalid_response', message: 'Invalid discard review path', retryable: false })
      let path: string
      try { path = typeof entry.path === 'string' ? entry.path : new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(new Uint8Array(entry.path.bytes)) }
      catch { throw new BackendError({ code: 'unsupported_path', message: 'An affected filename cannot be represented safely; review it using the command line', retryable: false }) }
      if (!path || path.includes('\0') || seen.has(path)) throw new BackendError({ code: 'invalid_response', message: 'Invalid discard review path', retryable: false })
      seen.add(path)
      return path
    })
    if (paths.some(path => !seen.has(path))) throw new BackendError({ code: 'invalid_response', message: 'Discard review omitted a selected path', retryable: false })
    return { basis: value.basis, files }
  }

  async repositoryDiscardBatch(repositoryPath: string, paths: ReadonlyArray<string>, basis: string): Promise<string> {
    let result: unknown
    try { result = await this.repositoryRendererRequest(repositoryPath, 'discard_batch', { paths, basis }) }
    catch (error) { const cause = toBackendError(error); throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the files and recovery history before retrying`, retryable: false, recoveryId: cause.recoveryId }) }
    if (!isRecord(result) || result.code !== 0 || typeof result.undo_id !== 'string' || !/^[a-f0-9-]{36}$/.test(result.undo_id)) throw new BackendError({ code: 'invalid_response', message: 'Discard outcome is uncertain; inspect the files and recovery history', retryable: false })
    return result.undo_id
  }

  async repositoryDiscardHistory(repositoryPath: string): Promise<ReadonlyArray<IDiscardRecoveryRecord>> {
    const value = await this.repositoryRendererRequest(repositoryPath, 'list_discards', {})
    const invalid = () => new BackendError({ code: 'invalid_response', message: 'Invalid discard recovery history', retryable: false })
    if (!Array.isArray(value) || value.length > 1000) throw invalid()
    const ids = new Set<string>()
    return value.map(entry => {
      if (!isRecord(entry) || typeof entry.id !== 'string' || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(entry.id) || ids.has(entry.id) || typeof entry.state !== 'string' || !['applied', 'prepared', 'undone', 'invalid'].includes(entry.state) || typeof entry.created_ns !== 'number' || !Number.isFinite(entry.created_ns) || entry.created_ns < 0 || entry.created_ns / 1_000_000 > 8_640_000_000_000_000) throw invalid()
      ids.add(entry.id)
      if (entry.state === 'invalid') return { id: entry.id, state: 'invalid', createdMs: 0, purpose: 'unknown', files: [], unavailableLfs: 0 }
      if (!Array.isArray(entry.files) || !entry.files.every(file => isRecord(file) && typeof file.display_path === 'string') || typeof entry.purpose !== 'string' || !['discard', 'conflict_choice', 'conflict_stage'].includes(entry.purpose) || typeof entry.lfs_payloads_unavailable !== 'number' || !Number.isSafeInteger(entry.lfs_payloads_unavailable) || entry.lfs_payloads_unavailable < 0) throw invalid()
      return { id: entry.id, state: entry.state as IDiscardRecoveryRecord['state'], createdMs: Math.floor(entry.created_ns / 1_000_000), purpose: entry.purpose as IDiscardRecoveryRecord['purpose'], files: entry.files.map(file => file.display_path as string), unavailableLfs: entry.lfs_payloads_unavailable }
    })
  }

  async repositoryWorkingDiff(repositoryPath: string, path: string): Promise<ICopilotCommitDiff> {
    const result = await this.repositoryRendererRequest(repositoryPath, 'diff_detail', { path, staged: false })
    if (!isCopilotCommitDiff(result)) throw new BackendError({ code: 'invalid_response', message: 'Invalid working diff response', retryable: false })
    return result
  }

  async repositoryStageSelectedLines(repositoryPath: string, path: string, basis: string, lines: ReadonlyArray<number>): Promise<void> {
    return this.repositoryStageLines(repositoryPath, path, undefined, basis, lines)
  }

  async repositoryStageRenamedLines(repositoryPath: string, path: string, originalPath: string, basis: string, lines: ReadonlyArray<number>, preserveSource = false): Promise<void> {
    return this.repositoryStageLines(repositoryPath, path, originalPath, basis, lines, preserveSource)
  }

  private async repositoryStageLines(repositoryPath: string, path: string, originalPath: string | undefined, basis: string, lines: ReadonlyArray<number>, preserveSource = false): Promise<void> {
    if (!/^[a-f0-9]{64}$/.test(basis) || lines.length === 0 || lines.some(line => !Number.isSafeInteger(line) || line < 0) || new Set(lines).size !== lines.length) throw new BackendError({ code: 'invalid_request', message: 'Invalid line selection', retryable: false })
    const method = originalPath === undefined ? 'select_diff' : 'select_renamed_diff'
    const params = originalPath === undefined ? { path, staged: false, basis, lines } : { path, original_path: originalPath, basis, lines, preserve_source: preserveSource }
    const size = new TextEncoder().encode(JSON.stringify({ method, ...params, id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length
    if (size > (this.transport === 'ws' ? 65_536 : 1_048_576)) throw new BackendError({ code: 'invalid_request', message: 'Line selection exceeds the request limit', retryable: false })
    let result: unknown
    try { result = await this.repositoryRendererRequest(repositoryPath, method, params) }
    catch (error) { const cause = toBackendError(error); throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the index before retrying`, retryable: false }) }
    if (!isRecord(result) || result.code !== 0 || typeof result.stdout !== 'string' || typeof result.stderr !== 'string') throw new BackendError({ code: 'invalid_response', message: 'Line staging outcome is uncertain; inspect the index', retryable: false })
  }

  async repositoryDiscardSelection(repositoryPath: string, path: string, selection: { readonly basis: string; readonly lines: ReadonlyArray<number> }): Promise<string> {
    const params = { path, scope: 'working_tree', selection }
    if (this.transport === 'ws' && new TextEncoder().encode(JSON.stringify({ ...params, method: 'discard', basis: 'f'.repeat(64), id: Number.MAX_SAFE_INTEGER, repository_id: 'f'.repeat(64) })).length > 65536) {
      throw new BackendError({ code: 'invalid_request', message: 'The selected lines exceed the bridge request limit', retryable: false })
    }
    const preview = await this.repositoryRendererRequest(repositoryPath, 'discard_preview', params)
    if (!isRecord(preview) || typeof preview.basis !== 'string' || !/^[a-f0-9]{64}$/.test(preview.basis)) {
      throw new BackendError({ code: 'invalid_response', message: 'Invalid discard preview', retryable: false })
    }
    let result: unknown
    try {
      result = await this.repositoryRendererRequest(repositoryPath, 'discard', { ...params, basis: preview.basis })
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the file and recovery history before retrying`, retryable: false, recoveryId: cause.recoveryId })
    }
    if (!isRecord(result) || result.code !== 0 || typeof result.undo_id !== 'string' || !/^[a-f0-9-]{36}$/.test(result.undo_id)) {
      throw new BackendError({ code: 'invalid_response', message: 'Discard outcome is uncertain; inspect the file and recovery history', retryable: false })
    }
    return result.undo_id
  }

  async undoRepositoryDiscard(repositoryPath: string, undoId: string): Promise<void> {
    let result: unknown
    try {
      result = await this.repositoryRendererRequest(repositoryPath, 'undo_discard', { undo_id: undoId })
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect the file and recovery history before retrying`, retryable: false, recoveryId: cause.recoveryId })
    }
    if (!isRecord(result) || result.code !== 0) {
      throw new BackendError({ code: 'invalid_response', message: 'Undo outcome is uncertain; inspect the file and recovery history', retryable: false })
    }
  }

  async forgetRepositoryDiscard(repositoryPath: string, undoId: string): Promise<void> {
    let result: unknown
    try {
      result = await this.repositoryRendererRequest(repositoryPath, 'forget_discard', { undo_id: undoId })
    } catch (error) {
      const cause = toBackendError(error)
      throw new BackendError({ code: cause.code, message: `${cause.message}; inspect recovery history before retrying`, retryable: false })
    }
    if (!isRecord(result) || result.code !== 0) {
      throw new BackendError({ code: 'invalid_response', message: 'Backup deletion outcome is uncertain; inspect recovery history', retryable: false })
    }
  }

  async repositoryStatusContext(repositoryPath: string, creation = false): Promise<{
    readonly mergeHeadFound: boolean
    readonly squashMsgFound: boolean
    readonly isCherryPickingHeadFound: boolean
    readonly rebaseInternalState: {
      readonly originalBranchTip: string
      readonly targetBranch: string
      readonly baseBranchTip: string
    } | null
  }> {
    const value = creation && this.transport === 'ws'
      ? await this.request('read_repository_creation_status', { path: repositoryPath, action: { type: 'context' } })
      : await this.repositoryRead(
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
    kind: 'merge' | 'rebase' | 'working',
    creation = false
  ): Promise<{
    readonly check: string
    readonly numstat: string
    readonly attributes: string
  }> {
    const value = creation && this.transport === 'ws'
      ? await this.request('read_repository_creation_status', { path: repositoryPath, action: { type: 'conflict_details', kind } })
      : await this.repositoryRead(
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
    options: BackendRequestOptions = {},
    creation = false
  ): Promise<ReadonlyArray<DetailedStatusRecord>> {
    if (creation && this.transport === 'ws') {
      return this.request('read_repository_creation_status', { path: repositoryPath, action: { type: 'detailed', include_untracked: includeUntracked } }, options) as Promise<ReadonlyArray<DetailedStatusRecord>>
    }
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
