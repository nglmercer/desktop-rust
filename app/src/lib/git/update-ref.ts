import { Repository } from '../../models/repository'
import { mkdir } from '../../web/fs-promises'
import { dirname } from 'path'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import {
  acquireOwnedFileLock,
  IOwnedFileLease,
  releaseOwnedFileLock,
} from './owned-file-lock'

/** Liveness of owned ref/HEAD locks across asynchronous guarded work. */
export interface IRefLock {
  /** Aborted when the lock owner fails before guarded work settles. */
  readonly signal: AbortSignal
  /** Throw before a mutation if the prepared ref guard is no longer alive. */
  readonly assertHeld: () => void
}

/**
 * Compare-and-swap a ref after verifying state while Git holds its ref locks.
 *
 * Unlike a read followed by updateRef, this also lets the caller verify HEAD's
 * symbolic identity under Git's implicit HEAD lock. The verifier must only read
 * repository state; it must not try to acquire the same ref locks.
 *
 * A null new value deletes an existing ref with an old-value guard.
 */
export async function updateRefWithVerification(
  repository: Repository,
  ref: string,
  oldValue: string,
  newValue: string | null,
  reason: string,
  verify: () => Promise<void>
): Promise<void> {
  return preparedRefTransaction(
    repository,
    ref,
    oldValue,
    newValue,
    verify,
    reason
  )
}

/**
 * Hold parent-owned file-backend HEAD/ref locks during guarded non-ref work.
 *
 * Locks remain owned by this process until all awaited work settles, including
 * dispatched filesystem mutations. No Git child lifetime can release them.
 * No ref/reflog is written. Use a zero object ID for an unborn ref.
 * onLockAcquired registers ownership before stat, retaining initialization
 * cleanup for a later retry if identity or filesystem cleanup is unavailable.
 */
export async function withRefLock(
  repository: Repository,
  ref: string,
  value: string,
  action: (lock: IRefLock) => Promise<void>,
  onLockAcquired?: (lease: IOwnedFileLease) => void
): Promise<void> {
  if (ref !== 'HEAD' && !ref.startsWith('refs/heads/')) {
    throw new Error(
      'Parent-owned ref locks require HEAD or a fully qualified branch'
    )
  }
  const context = await backend().repositoryRefLockContext(
    repository.path,
    ref,
    value
  )
  const paths = context.paths
  const locks = []
  let active = true
  const controller = new AbortController()
  const guard: IRefLock = {
    signal: controller.signal,
    assertHeld: () => {
      if (!active || controller.signal.aborted) {
        throw new Error('Parent-owned HEAD/ref guard is no longer held', {
          cause: controller.signal.reason,
        })
      }
    },
  }
  let failure: unknown
  let failed = false
  try {
    for (const name of paths) {
      const path = `${name}.lock`
      await mkdir(dirname(path), { recursive: true })
      const owned = await acquireOwnedFileLock(path, undefined, onLockAcquired)
      locks.push(owned)
    }
    const lockedContext = await backend().repositoryRefLockContext(
      repository.path,
      ref,
      value
    )
    if (!lockedContext.matches) {
      throw new Error(`Ref changed before guarded index work: ${ref}`)
    }
    await action(guard)
  } catch (error) {
    failed = true
    failure = error
    controller.abort(error)
  } finally {
    active = false
    const cleanupErrors: unknown[] = []
    for (const lock of [...locks].reverse()) {
      try {
        await releaseOwnedFileLock(lock)
      } catch (error) {
        cleanupErrors.push(error)
      }
    }
    if (cleanupErrors.length > 0) {
      throw new AggregateError(
        failed ? [failure, ...cleanupErrors] : cleanupErrors,
        'Could not release parent-owned ref locks'
      )
    }
  }
  if (failed) {
    throw failure
  }
}

async function preparedRefTransaction(
  repository: Repository,
  ref: string,
  oldValue: string,
  newValue: string | null,
  action: (lock: IRefLock) => Promise<void>,
  reason?: string
): Promise<void> {
  let active = true
  const controller = new AbortController()
  const lock: IRefLock = {
    signal: controller.signal,
    assertHeld: () => {
      if (!active || controller.signal.aborted) {
        throw new Error('Git ref guard was lost during guarded work', {
          cause: controller.signal.reason,
        })
      }
    },
  }

  try {
    await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().repositoryUpdateRefWithVerification(
          repository.path,
          {
            reference: ref,
            old_value: oldValue,
            new_value: newValue,
            ...(reason === undefined ? {} : { reason }),
          },
          async () => {
            lock.assertHeld()
            await action(lock)
            lock.assertHeld()
          }
        ),
      true
    )
  } catch (error) {
    controller.abort(error)
    throw error
  } finally {
    active = false
  }
}

/**
 * Remove a ref.
 *
 * @param repository - The repository in which the ref exists.
 * @param ref        - The ref to remove. Should be fully qualified, but may also be 'HEAD'.
 * @param reason     - The reflog entry (optional). Note that this is only useful when
 *                     deleting the HEAD reference as deleting any other reference will
 *                     implicitly delete the reflog file for that reference as well.
 */
export async function deleteRef(
  repository: Repository,
  ref: string,
  reason?: string,
  expectedOid?: string
) {
  if (
    ref === 'HEAD' &&
    expectedOid !== undefined &&
    reason === 'Reverting first commit'
  ) {
    await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () => backend().repositoryDeleteInitialHead(repository.path, expectedOid),
      true
    )
    return
  }
  throw new Error('Only reviewed initial HEAD deletion is supported')
}
