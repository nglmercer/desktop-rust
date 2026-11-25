import {
  WorkingDirectoryFileChange,
  AppFileStatusKind,
} from '../../models/status'
import { DiffType, ITextDiff, ILargeTextDiff } from '../../models/diff'
import { Repository } from '../../models/repository'
import { getWorkingDirectoryDiff } from './diff'
import { assertNever } from '../fatal-error'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import { getReviewedDiffLines } from '../copilot/commit-selection'

/** Stage selected lines using their certified diff basis when supplied. */
export async function applyPatchToIndex(
  repository: Repository,
  file: WorkingDirectoryFileChange,
  certifiedDiff?: ITextDiff | ILargeTextDiff,
  preserveRenameSource = false
): Promise<void> {
  const diff =
    certifiedDiff ?? (await getWorkingDirectoryDiff(repository, file))

  if (diff.kind !== DiffType.Text && diff.kind !== DiffType.LargeText) {
    const { kind } = diff
    switch (diff.kind) {
      case DiffType.Binary:
      case DiffType.Submodule:
      case DiffType.Image:
        throw new Error(
          `Can't create partial commit in binary file: ${file.path}`
        )
      case DiffType.Unrenderable:
        throw new Error(
          `File diff is too large to generate a partial commit: ${file.path}`
        )
      default:
        assertNever(diff, `Unknown diff kind: ${kind}`)
    }
  }

  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    async () => {
      const originalPath =
        file.status.kind === AppFileStatusKind.Renamed
          ? file.status.oldPath
          : undefined
      const preview =
        originalPath === undefined
          ? await backend().repositoryWorkingDiff(repository.path, file.path)
          : await backend().copilotCommitDiffForRepository(
              repository.path,
              file.path,
              {},
              originalPath
            )
      const lines = getReviewedDiffLines(
        file.path,
        file.selection,
        diff,
        preview,
        'The selected diff changed. Review and reselect its lines before committing.'
      )
      if (originalPath === undefined) {
        await backend().repositoryStageSelectedLines(
          repository.path,
          file.path,
          preview.basis,
          lines
        )
      } else {
        await backend().repositoryStageRenamedLines(
          repository.path,
          file.path,
          originalPath,
          preview.basis,
          lines,
          preserveRenameSource
        )
      }
    },
    true
  )
}
