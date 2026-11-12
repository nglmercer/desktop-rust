/**
 * Generated-compatible contract types for the Rust `application` service.
 *
 * Single source: `rust/crates/core/src/application.rs` (`IRequest`,
 * `IBackendError`, `Capabilities`, `BACKEND_VERSION`). The TypeScript
 * renderer consumes these via `app/src/backend/client.ts`. The loopback
 * bridge and the limited Wry read path do not yet have full parity. Validated by
 * `cargo test -p desktop-core application` and `cargo clippy`; check
 * freshness with `cargo test -p desktop-core --locked` after changing
 * `application.rs`.
 */

/** Backend protocol version. The bridge advertises `3` on connect (not JSON-RPC v1/v2). */
export const BACKEND_VERSION = 3 as const

/** A Git path or header value with its original bytes intact. */
export type GitBytes = { readonly bytes: ReadonlyArray<number> }

/** Git's disambiguated branch names and canonical references. */
export interface IBackendBranch {
  readonly ref: string
  readonly name: string
  readonly upstream: string | null
  readonly upstream_ref: string | null
  readonly current: boolean
  readonly oid: string
}

export interface IBackendBranchRef {
  readonly ref: string
  readonly oid: string
}

/** Porcelain-v2 records returned by the Rust detailed status operation. */
export type DetailedStatusRecord =
  | { readonly kind: 'header'; readonly value: GitBytes }
  | {
      readonly kind: 'entry'
      readonly entry_kind: '1' | '2' | 'u' | '?'
      readonly status_code: string
      readonly submodule_status_code: string
      readonly path: GitBytes
      readonly old_path: GitBytes | null
      readonly rename_or_copy_score: number | null
    }

/** Declared capabilities the backend advertises on connect. */
export interface Capabilities {
  /** Cooperative cancellation via `cancel(requestId)` / `cancel` RPC. */
  readonly cancel: boolean
  /** Max pending RPCs (bridge 32) / IPC promises on webview (profile-dependent). */
  readonly max_pending: number
}

/** Common request envelope — maps to Rust `IRequest`. */
export interface IRequest<T = unknown> {
  readonly sessionId: string
  readonly requestId: string
  readonly method: string
  readonly repositoryId?: string
  readonly accountId?: string
  readonly deadlineMs?: number
  readonly params: T
}

/** Uniform backend error — maps to Rust `IBackendError`. */
export interface IBackendError {
  readonly code: string
  readonly message: string
  readonly retryable: boolean
  readonly jobId?: string
  readonly recoveryId?: string
  readonly details?: Readonly<Record<string, unknown>>
}

/** Terminal job lifecycle: `queued → running → settling → terminal`. Exactly one terminal. */
export type JobTerminal =
  | { readonly kind: 'success'; readonly value: unknown }
  | { readonly kind: 'error'; readonly error: IBackendError }
  | { readonly kind: 'cancelled' }
  | { readonly kind: 'indeterminate'; readonly error: IBackendError }

/** Wire `ready` payload from the bridge (also negotiated over Wry IPC). */
export interface ReadyPayload {
  readonly ready: true
  readonly version: typeof BACKEND_VERSION
  readonly capabilities: Capabilities
}
