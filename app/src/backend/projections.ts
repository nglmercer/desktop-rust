/**
 * React projections — adapters that present backend state to the renderer.
 *
 * `AppStore`, `GitStore`, `RepositoriesStore`, `AccountsStore` and the
 * caches/dispatcher are evolving into projections of the Rust service
 * (`rust/crates/core`). Interfaces stay so components keep working; the
 * authority (effects, persistence, credentials, Git) stays in Rust.
 *
 * Each projection wraps a backend slice with a monotonic revision and a
 * repository/account scope; stale results after selection/identity changes
 * are discarded. Deltas are bounded: history/diffs/PRs/issues are paged
 * (see `PagedParams`). Snapshot/projection of the UI is cached, but no
 * projection ever decides transactions, resolves credentials or owns persistence.
 */

import { backend, BackendError } from './client'
import type { IRequest } from './contracts'

/** Monotonic revision guard for a projection key. */
export class RevisionTracker {
  private revision = 0
  private scope: { repositoryId?: string; accountId?: string } | null = null

  /** Bump on every new fetch; returns the revision to tag the response with. */
  begin(scope?: { repositoryId?: string; accountId?: string }): number {
    this.scope = scope ?? this.scope
    return ++this.revision
  }

  /** Returns true when a response tagged `revision` is still current for `scope`. */
  isCurrent(revision: number, scope?: { repositoryId?: string; accountId?: string }): boolean {
    if (scope) {
      const expected = scope
      if (this.scope?.repositoryId !== expected.repositoryId || this.scope?.accountId !== expected.accountId) {
        return false
      }
    }
    return revision === this.revision
  }

  invalidate(): void {
    ++this.revision
    this.scope = null
  }
}

export interface PagedParams {
  readonly offset?: number
  readonly limit?: number
}

export interface PendingSettings {
  /** Server-advertised max_pending for this transport/profile (64 in low-memory). */
  readonly maxPending: number
}

/** Base projection: holds revision + scope, discards stale responses, never replays mutations on reconnect. */
export abstract class BackendProjection<T> {
  protected revision = new RevisionTracker()
  protected current: T | null = null
  protected epoch = 0

  protected abstract key(scope?: { repositoryId?: string; accountId?: string }): string

  /** Fetch a snapshot through the backend; resolves only if still current. */
  async fetch<TResponse>(method: string, params: PagedParams = {}, scope?: { repositoryId?: string; accountId?: string }): Promise<TResponse | null> {
    const rev = this.revision.begin(scope)
    const reqParams = { ...params } as Record<string, unknown>
    const opts: { repositoryId?: string; accountId?: string } = {}
    if (scope?.repositoryId) opts.repositoryId = scope.repositoryId
    if (scope?.accountId) opts.accountId = scope.accountId
    try {
      const res = await backend().request<TResponse>(method, reqParams, opts)
      if (!this.revision.isCurrent(rev, scope)) return null
      return res
    } catch (error) {
      if (error instanceof BackendError && error.code === 'cancelled') return null
      throw error
    }
  }

  /** On reconnect we refresh snapshot + known jobs but never replay commits/pushes/publication/other mutating RPCs. */
  async resync(): Promise<void> {
    // No-op by default; subclasses refresh their snapshot. Mutations are
    // never re-issued automatically — the Rust side is idempotent via `basis`.
  }
}

/** Repository list projection (formerly `RepositoriesStore` Dexie-backed). */
export class RepositoriesProjection extends BackendProjection<{ version: number; items: Array<{ path: string }> }> {
  protected key(): string {
    return 'repositories'
  }

  async list(): Promise<Array<{ path: string }> | null> {
    const res = await this.fetch<{ repositories: Array<{ path: string }>; version: number }>('list_repositories', {})
    if (!res) return null
    this.current = { version: res.version, items: res.repositories }
    return res.repositories
  }
}

/** History/diff PR/issues projections are paged (bounded deltas). */
export function pageHistory<T>(offset: number, limit: number): PagedParams {
  return { offset, limit }
}

/**
 * Guard that prevents the renderer from deciding effects. Repository spend
 * still enforced in Rust (` basis` revision checks, `checkout_preview`
 * → `checkout` with `basis`). The projections throw a `BackendError` rather
 * than a shell-built `git` argument to make misuse loud.
 */
export function assertNotBuildingGitArgs(): never {
  throw new BackendError({
    code: 'renderer_forbidden',
    message: 'Renderer must not build Git arguments — delegate to the Rust service via `backend().request()`',
    retryable: false,
  })
}
