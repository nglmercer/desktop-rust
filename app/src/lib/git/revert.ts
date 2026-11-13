import { git, IGitStringExecutionOptions } from './core'

import { Repository } from '../../models/repository'
import { Commit } from '../../models/commit'
import { IRevertProgress } from '../../models/progress'

import { executionOptionsWithProgress } from '../progress/from-process'
import { RevertProgressParser } from '../progress/revert'
import {
  envForRemoteOperation,
  getFallbackUrlForProxyResolve,
} from './environment'
import { IRemote } from '../../models/remote'
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
  currentRemote: IRemote | null,
  progressCallback?: (progress: IRevertProgress) => void,
  expected?: { readonly head: string; readonly branch: string | null }
) {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    if (expected === undefined) throw new Error('Refresh the current branch before reverting')
    progressCallback?.({ kind: 'revert', title: 'Reverting commit', description: 'Reverting changes', value: 0 })
    await withRepositoryGitOperation(repository.path, 'mutation', () => backend().revertReviewedCommit(repository.path, commit.sha, { ...expected, merge: commit.parentSHAs.length > 1 }), true)
    progressCallback?.({ kind: 'revert', title: 'Reverting commit', description: 'Revert complete', value: 1 })
    return
  }
  const args = ['revert']
  if (commit.parentSHAs.length > 1) {
    args.push('-m', '1')
  }

  args.push(commit.sha)

  let opts: IGitStringExecutionOptions = {}
  if (progressCallback) {
    const env = await envForRemoteOperation(
      getFallbackUrlForProxyResolve(repository, currentRemote)
    )
    opts = await executionOptionsWithProgress(
      { env, trackLFSProgress: true },
      new RevertProgressParser(),
      progress => {
        const description =
          progress.kind === 'progress' ? progress.details.text : progress.text
        const title = progress.kind === 'progress' ? progress.details.title : ''
        const value = progress.percent

        progressCallback({ kind: 'revert', description, value, title })
      }
    )
  }

  await git(args, repository.path, 'revert', opts)
}
