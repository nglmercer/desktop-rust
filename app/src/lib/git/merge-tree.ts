import type { Branch } from '../../models/branch'
import { ComputedAction } from '../../models/computed-action'
import type { MergeTreeResult } from '../../models/merge'
import type { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

export async function determineMergeability(
  repository: Repository,
  ours: Branch,
  theirs: Branch
): Promise<MergeTreeResult> {
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
