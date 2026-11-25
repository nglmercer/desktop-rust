import { Repository } from '../../models/repository'
import { Commit } from '../../models/commit'
import { IRevertProgress } from '../../models/progress'
import type { IRemote } from '../../models/remote'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Creates a new commit that reverts the changes of a previous commit
 *
 * @param repository  - The repository to update
 *
 * @param commit         - The SHA of the commit to be reverted
 */
export async function revertCommit(
  repository: Repository,
  commit: Commit,
  _currentRemote: IRemote | null,
  progressCallback?: (progress: IRevertProgress) => void,
  expected?: { readonly head: string; readonly branch: string | null }
) {
  if (expected === undefined)
    throw new Error('Refresh the current branch before reverting')
  progressCallback?.({
    kind: 'revert',
    title: 'Reverting commit',
    description: 'Reverting changes',
    value: 0,
  })
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().revertReviewedCommit(repository.path, commit.sha, {
        ...expected,
        merge: commit.parentSHAs.length > 1,
      }),
    true
  )
  progressCallback?.({
    kind: 'revert',
    title: 'Reverting commit',
    description: 'Revert complete',
    value: 1,
  })
}
