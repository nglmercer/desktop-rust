import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/** The reset modes which are supported. */
export const enum GitResetMode {
  /**
   * Resets the index and working tree. Any changes to tracked files in the
   * working tree since <commit> are discarded.
   */
  Hard = 0,
  /**
   * Does not touch the index file or the working tree at all (but resets the
   * head to <commit>, just like all modes do). This leaves all your changed
   * files "Changes to be committed", as git status would put it.
   */
  Soft,

  /**
   * Resets the index but not the working tree (i.e., the changed files are
   * preserved but not marked for commit) and reports what has not been updated.
   * This is the default action for git reset.
   */
  Mixed,
}

/** Reset with the mode to the ref. */
export async function reset(
  repository: Repository,
  mode: GitResetMode,
  ref: string,
  expected?: {
    readonly head: string
    readonly branch: string | null
    readonly undo: boolean
  }
): Promise<true> {
  if (mode === GitResetMode.Hard || mode === GitResetMode.Soft) {
    await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().resetRepository(
          repository.path,
          ref,
          mode === GitResetMode.Hard ? 'hard' : 'soft'
        ),
      true
    )
    return true
  }
  if (mode === GitResetMode.Mixed) {
    if (expected === undefined)
      throw new Error('Refresh the current branch before resetting')
    await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().mixedResetRepository(repository.path, {
          target: ref,
          ...expected,
        }),
      true
    )
    return true
  }
  throw new Error(`Unknown reset mode: ${mode}`)
}

/** Unstage all paths. */
export async function unstageAll(repository: Repository): Promise<true> {
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().repositoryUnstageAll(repository.path),
    true
  )
  return true
}
