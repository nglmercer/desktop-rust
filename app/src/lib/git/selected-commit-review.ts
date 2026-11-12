import {
  backend,
  type ICopilotCommitOptions,
  type ISelectedCommitReview,
} from '../../backend/client'
import {
  DiffSelectionType,
  type ILargeTextDiff,
  type ITextDiff,
} from '../../models/diff'
import type { Repository } from '../../models/repository'
import {
  AppFileStatusKind,
  type WorkingDirectoryFileChange,
} from '../../models/status'
import {
  getReviewedDiffLines,
  getWholeFileCopilotSelection,
} from '../copilot/commit-selection'

/** Review the exact selected Git blobs without touching the live index. */
export async function reviewSelectedCommit(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  displayedDiff: (
    file: WorkingDirectoryFileChange
  ) => ITextDiff | ILargeTextDiff | undefined,
  amendHead?: string,
  expectedHead?: ICopilotCommitOptions['expected_head'],
  allowEmpty = false
): Promise<ISelectedCommitReview> {
  const partial = files.filter(
    file => file.selection.getSelectionType() === DiffSelectionType.Partial
  )
  const whole = getWholeFileCopilotSelection(
    files.filter(
      file => file.selection.getSelectionType() !== DiffSelectionType.Partial
    )
  )
  if (whole === null)
    throw new Error('Conflicted files cannot be reviewed as a selected commit.')
  const partialSelections: NonNullable<
    ICopilotCommitOptions['partial_selections']
  >[number][] = []
  for (const file of partial) {
    const displayed = displayedDiff(file)
    if (
      displayed === undefined ||
      file.status.kind === AppFileStatusKind.Conflicted ||
      file.status.submoduleStatus !== undefined
    ) {
      throw new Error(
        'The selected diff is no longer available. Review and reselect its lines.'
      )
    }
    const oldPath =
      file.status.kind === AppFileStatusKind.Renamed
        ? file.status.oldPath
        : undefined
    const fresh = await backend().copilotCommitDiffForRepository(
      repository.path,
      file.path,
      {},
      oldPath
    )
    partialSelections.push({
      path: file.path,
      ...(oldPath === undefined ? {} : { original_path: oldPath }),
      basis: fresh.basis,
      lines: getReviewedDiffLines(
        file.path,
        file.selection,
        displayed,
        fresh,
        'The selected diff changed. Review and reselect its lines before committing.'
      ),
    })
  }
  return backend().repositorySelectedCommitPreview(repository.path, {
    paths: whole.paths,
    remove_paths: whole.removePaths,
    partial_selections: partialSelections,
    allow_empty: allowEmpty,
    amend_head: amendHead,
    expected_head: expectedHead,
  })
}
