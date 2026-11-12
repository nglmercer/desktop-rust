import { BackendGitPath, ICopilotCommitDiff } from '../../backend/client'
import {
  DiffLineType,
  DiffSelectionType,
  ILargeTextDiff,
  ITextDiff,
} from '../../models/diff'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../models/status'

/** Translate whole-file selections to the private Rust index snapshot. */
export function getWholeFileCopilotSelection(
  files: ReadonlyArray<WorkingDirectoryFileChange>
): {
  readonly paths: ReadonlyArray<string>
  readonly removePaths: ReadonlyArray<string>
} | null {
  const paths: string[] = []
  const removePaths: string[] = []
  for (const file of files) {
    const selection = file.selection.getSelectionType()
    if (selection === DiffSelectionType.None) {
      continue
    }
    if (
      selection !== DiffSelectionType.All ||
      file.status.kind === AppFileStatusKind.Conflicted
    ) {
      return null
    }
    if (file.status.kind === AppFileStatusKind.Deleted) {
      removePaths.push(file.path)
    } else {
      paths.push(file.path)
      if (file.status.kind === AppFileStatusKind.Renamed) {
        removePaths.push(file.status.oldPath)
      }
    }
  }
  return { paths, removePaths: [...new Set(removePaths)] }
}

/** Match a retained display exactly to a fresh Rust preview before using its IDs. */
function matchesPath(path: BackendGitPath, expected: string): boolean {
  if (typeof path === 'string') {
    return path === expected
  }
  const bytes = new TextEncoder().encode(expected)
  return (
    bytes.length === path.bytes.length &&
    bytes.every((byte, i) => byte === path.bytes[i])
  )
}

export function getReviewedCopilotLines(
  file: WorkingDirectoryFileChange,
  displayed: ITextDiff | ILargeTextDiff,
  preview: ICopilotCommitDiff
): ReadonlyArray<number> {
  const stale = () =>
    new Error(
      'The selected diff changed. Review and reselect its lines before generating a commit message.'
    )
  if (
    !matchesPath(preview.path, file.path) ||
    !preview.can_select ||
    preview.hunks.length !== displayed.hunks.length
  ) {
    throw stale()
  }
  const selected: number[] = []
  const ids = new Set<number>()
  for (const [hunkIndex, hunk] of displayed.hunks.entries()) {
    const fresh = preview.hunks[hunkIndex]
    if (
      hunk.header.oldStartLine !== fresh.old_start ||
      hunk.header.oldLineCount !== fresh.old_count ||
      hunk.header.newStartLine !== fresh.new_start ||
      hunk.header.newLineCount !== fresh.new_count ||
      hunk.lines.length !== fresh.lines.length + 1 ||
      hunk.lines[0]?.type !== DiffLineType.Hunk
    ) {
      throw stale()
    }
    for (const [index, line] of hunk.lines.slice(1).entries()) {
      const next = fresh.lines[index]
      const kind =
        line.type === DiffLineType.Add
          ? 'added'
          : line.type === DiffLineType.Delete
          ? 'deleted'
          : line.type === DiffLineType.Context
          ? 'context'
          : null
      if (
        kind === null ||
        kind !== next.kind ||
        line.content !== next.text ||
        line.oldLineNumber !== next.old_line ||
        line.newLineNumber !== next.new_line ||
        line.noTrailingNewLine !== next.no_newline ||
        ids.has(next.id)
      ) {
        throw stale()
      }
      ids.add(next.id)
      if (
        kind !== 'context' &&
        file.selection.isSelected(hunk.unifiedDiffStart + index + 1)
      ) {
        selected.push(next.id)
      }
    }
  }
  if (selected.length === 0) {
    throw stale()
  }
  return selected
}
