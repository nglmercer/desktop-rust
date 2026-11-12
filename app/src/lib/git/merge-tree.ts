import type { Branch } from '../../models/branch'
import { ComputedAction } from '../../models/computed-action'
import type { MergeTreeResult } from '../../models/merge'
import type { Repository } from '../../models/repository'
import { git, isGitError } from './core'
import { GitError } from 'dugite'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

export async function determineMergeability(
  repository: Repository,
  ours: Branch,
  theirs: Branch
): Promise<MergeTreeResult> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    const result = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().repositoryMergeTreePreview(
          repository.path,
          ours.tip.sha,
          theirs.tip.sha
        ),
      true
    )
    if (result.kind === 'conflicts')
      return {
        kind: ComputedAction.Conflicts,
        conflictedFiles: result.conflictedFiles,
      }
    return {
      kind:
        result.kind === 'clean' ? ComputedAction.Clean : ComputedAction.Invalid,
    }
  }
  return git(
    [
      'merge-tree',
      '--write-tree',
      '--name-only',
      '--no-messages',
      '-z',
      ours.tip.sha,
      theirs.tip.sha,
    ],
    repository.path,
    'determineMergeability',
    { successExitCodes: new Set([0, 1]) }
  )
    .then<MergeTreeResult>(({ stdout }) => {
      // The output will be "<tree-id>\0[<filename>\0]*" so we can get the
      // number of conflicted files by counting the number of null bytes and
      // subtracting one for the tree id.
      const conflictedFiles = (stdout.match(/\0/g)?.length ?? 0) - 1
      return conflictedFiles > 0
        ? { kind: ComputedAction.Conflicts, conflictedFiles }
        : { kind: ComputedAction.Clean }
    })
    .catch<MergeTreeResult>(e =>
      isGitError(e, GitError.CannotMergeUnrelatedHistories)
        ? Promise.resolve({ kind: ComputedAction.Invalid })
        : Promise.reject(e)
    )
}
