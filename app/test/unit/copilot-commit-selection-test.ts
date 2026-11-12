import { describe, expect, it } from 'vitest'
import {
  getReviewedCopilotLines,
  getWholeFileCopilotSelection,
} from '../../src/lib/copilot/commit-selection'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffSelection,
  DiffSelectionType,
  DiffType,
  ITextDiff,
} from '../../src/models/diff'
import { ICopilotCommitDiff } from '../../src/backend/client'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'

const all = DiffSelection.fromInitialSelection(DiffSelectionType.All)
const none = DiffSelection.fromInitialSelection(DiffSelectionType.None)

describe('Copilot whole-file selections', () => {
  it('removes renamed and deleted paths without including excluded files', () => {
    const selected = getWholeFileCopilotSelection([
      new WorkingDirectoryFileChange(
        'new',
        {
          kind: AppFileStatusKind.Renamed,
          oldPath: 'old',
          renameIncludesModifications: false,
        },
        all
      ),
      new WorkingDirectoryFileChange(
        'deleted',
        { kind: AppFileStatusKind.Deleted },
        all
      ),
      new WorkingDirectoryFileChange(
        'old',
        { kind: AppFileStatusKind.Untracked },
        all
      ),
      new WorkingDirectoryFileChange(
        'excluded',
        { kind: AppFileStatusKind.Modified },
        none
      ),
    ])
    expect(selected).toEqual({
      paths: ['new', 'old'],
      removePaths: ['old', 'deleted'],
    })
  })

  it('does not represent a partial selection as the whole file', () => {
    const partial = all.withLineSelection(1, false)
    const file = new WorkingDirectoryFileChange(
      'partial',
      { kind: AppFileStatusKind.Modified },
      partial
    )
    expect(getWholeFileCopilotSelection([file])).toBeNull()
  })

  it('maps only selected reviewed lines across multiple hunks', () => {
    const raw = new DiffParser().parse(`diff --git a/partial b/partial
--- a/partial
+++ b/partial
@@ -1,2 +1,2 @@
 context
-old
+new
@@ -10 +10 @@
-last
+final
\\ No newline at end of file
`)
    const displayed: ITextDiff = {
      kind: DiffType.Text,
      text: raw.contents,
      hunks: raw.hunks,
      maxLineNumber: raw.maxLineNumber,
      hasHiddenBidiChars: false,
    }
    const file = new WorkingDirectoryFileChange(
      'partial',
      { kind: AppFileStatusKind.Modified },
      none.withLineSelection(2, true).withLineSelection(6, true)
    )
    const preview: ICopilotCommitDiff = {
      path: 'partial',
      basis: 'basis',
      kind: 'text',
      can_select: true,
      hunks: [
        {
          old_start: 1,
          old_count: 2,
          new_start: 1,
          new_count: 2,
          lines: [
            {
              id: 0,
              kind: 'context',
              text: 'context',
              old_line: 1,
              new_line: 1,
              no_newline: false,
            },
            {
              id: 1,
              kind: 'deleted',
              text: 'old',
              old_line: 2,
              new_line: null,
              no_newline: false,
            },
            {
              id: 2,
              kind: 'added',
              text: 'new',
              old_line: null,
              new_line: 2,
              no_newline: false,
            },
          ],
        },
        {
          old_start: 10,
          old_count: 1,
          new_start: 10,
          new_count: 1,
          lines: [
            {
              id: 3,
              kind: 'deleted',
              text: 'last',
              old_line: 10,
              new_line: null,
              no_newline: false,
            },
            {
              id: 4,
              kind: 'added',
              text: 'final',
              old_line: null,
              new_line: 10,
              no_newline: true,
            },
          ],
        },
      ],
    }
    expect(getReviewedCopilotLines(file, displayed, preview)).toEqual([1, 4])
    expect(
      getReviewedCopilotLines(file, displayed, {
        ...preview,
        path: { bytes: [...new TextEncoder().encode('partial')] },
      })
    ).toEqual([1, 4])
    const changedNewline = {
      ...preview,
      hunks: [
        preview.hunks[0],
        {
          ...preview.hunks[1],
          lines: [
            preview.hunks[1].lines[0],
            { ...preview.hunks[1].lines[1], no_newline: false },
          ],
        },
      ],
    }
    expect(() =>
      getReviewedCopilotLines(file, displayed, changedNewline)
    ).toThrow('selected diff changed')
    expect(() =>
      getReviewedCopilotLines(file, displayed, { ...preview, path: 'other' })
    ).toThrow('selected diff changed')
    const changedLine = {
      ...preview,
      hunks: [
        {
          ...preview.hunks[0],
          lines: [
            preview.hunks[0].lines[0],
            { ...preview.hunks[0].lines[1], text: 'changed' },
            preview.hunks[0].lines[2],
          ],
        },
        preview.hunks[1],
      ],
    }
    expect(() => getReviewedCopilotLines(file, displayed, changedLine)).toThrow(
      'selected diff changed'
    )
  })
})
