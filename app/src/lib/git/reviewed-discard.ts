import { backend } from '../../backend/client'
import {
  type DiffSelection,
  DiffSelectionType,
  type ITextDiff,
} from '../../models/diff'
import type { Repository } from '../../models/repository'
import { getReviewedDiffLines } from '../copilot/commit-selection'

/** Discard only the displayed lines after Rust certifies their current basis. */
export async function discardReviewedSelection(
  repository: Repository,
  path: string,
  diff: ITextDiff,
  selection: DiffSelection
): Promise<string | null> {
  if (selection.getSelectionType() === DiffSelectionType.None) return null
  const client = backend()
  const preview = await client.repositoryWorkingDiff(repository.path, path)
  const lines = getReviewedDiffLines(
    path,
    selection,
    diff,
    preview,
    'The selected diff changed. Review and reselect its lines before discarding.'
  )
  return client.repositoryDiscardSelection(repository.path, path, {
    basis: preview.basis,
    lines,
  })
}

/** Honor disabled confirmation only when the reviewed operation affects exactly the selected paths. */
export async function discardBatchWithoutDialog(
  repository: Repository,
  paths: ReadonlyArray<string>
): Promise<string> {
  const client = backend()
  const preview = await client.repositoryDiscardBatchPreview(
    repository.path,
    paths
  )
  const requested = new Set(paths)
  if (preview.files.some(path => !requested.has(path))) {
    throw new Error(
      'Additional files are affected. Review the discard before continuing.'
    )
  }
  return client.repositoryDiscardBatch(repository.path, paths, preview.basis)
}
