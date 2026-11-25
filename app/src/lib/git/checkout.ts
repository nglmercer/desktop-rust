import { Repository } from '../../models/repository'
import { Branch } from '../../models/branch'
import { ICheckoutProgress } from '../../models/progress'
import { CommitOneLine, shortenSHA } from '../../models/commit'
import type { IRemote } from '../../models/remote'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import { batchIndexPaths } from './checkout-index'

export type ProgressCallback = (progress: ICheckoutProgress) => void

export async function checkoutBranch(
  repository: Repository,
  branch: Branch,
  _currentRemote: IRemote | null,
  progressCallback?: ProgressCallback,
  allowFileProtocol: boolean = false
): Promise<true> {
  const title = `Checking out branch ${branch.name}`
  progressCallback?.({
    kind: 'checkout',
    title,
    description: `Switching to ${__DARWIN__ ? 'Branch' : 'branch'}`,
    value: 0,
    target: branch.name,
  })
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().checkoutDirectBranch(
        repository.path,
        branch.ref,
        branch.tip.sha,
        allowFileProtocol
      ),
    true
  )
  progressCallback?.({
    kind: 'checkout',
    title,
    description: 'Submodules updated',
    value: 1,
    target: branch.name,
  })
  return true
}

export async function checkoutCommit(
  repository: Repository,
  commit: CommitOneLine,
  _currentRemote: IRemote | null,
  progressCallback?: ProgressCallback,
  allowFileProtocol: boolean = false
): Promise<true> {
  const title = `Checking out ${__DARWIN__ ? 'Commit' : 'commit'}`
  const target = shortenSHA(commit.sha)
  progressCallback?.({
    kind: 'checkout',
    title,
    description: title,
    value: 0,
    target,
  })
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().checkoutDetachedCommit(
        repository.path,
        commit.sha,
        allowFileProtocol
      ),
    true
  )
  progressCallback?.({
    kind: 'checkout',
    title,
    description: 'Submodules updated',
    value: 1,
    target,
  })
  return true
}

/** Check out the paths at HEAD. */
export async function checkoutPaths(
  repository: Repository,
  paths: ReadonlyArray<string>
): Promise<void> {
  const batches = batchIndexPaths(paths)
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    async () => {
      if (batches.length === 0) {
        await backend().repositoryCheckoutHeadPaths(repository.path, [])
      }
      for (const selected of batches) {
        await backend().repositoryCheckoutHeadPaths(repository.path, selected)
      }
    },
    true
  )
}
