import { describe, expect, it, vi } from 'vitest'

vi.mock('../../src/lib/git', () => ({}))
vi.mock('../../src/lib/git/assisted-commit', () => ({
  readAssistedCommitHead: vi.fn(async () => ({
    ref: 'refs/heads/main',
    sha: null,
  })),
  AssistedCommitError: class extends Error {},
}))
vi.mock('../../src/lib/git/repository-operation', () => ({}))
vi.mock('../../src/lib/assisted-commit-run', () => ({}))
vi.mock('../../src/lib/assisted-commit-request', () => ({
  getAssistedCommitInputVersions: vi.fn(async () => ['verified-version']),
}))

import {
  AssistedCommitSelection,
  sameSelectionDiff,
} from '../../src/lib/assisted-commit/selection'
import { ChangesSelectionKind } from '../../src/lib/app-state'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffHunk,
  DiffSelection,
  DiffSelectionType,
  DiffType,
} from '../../src/models/diff'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'
import { Repository } from '../../src/models/repository'
import { TipState } from '../../src/models/tip'

describe('reviewed Copilot selection basis', () => {
  it('retains the exact displayed diff through derived and restored selections', async () => {
    const parsed = new DiffParser().parse(`diff --git a/file b/file
--- a/file
+++ b/file
@@ -1 +1 @@
-old
+new
\\ No newline at end of file
`)
    const diff = {
      kind: DiffType.Text as const,
      text: parsed.contents,
      hunks: parsed.hunks,
      maxLineNumber: parsed.maxLineNumber,
      hasHiddenBidiChars: false,
    }
    const initial = DiffSelection.fromInitialSelection(DiffSelectionType.None)
    const file = new WorkingDirectoryFileChange(
      'file',
      { kind: AppFileStatusKind.Modified },
      initial
    )
    const state = {
      changesState: {
        selection: {
          kind: ChangesSelectionKind.WorkingDirectory,
          selectedFileIDs: [file.id],
          diff,
        },
        workingDirectory: { findFileWithID: () => file },
      },
    }
    const cache = { get: () => state }
    const selections = new AssistedCommitSelection(cache as never, () => false)
    const repository = {} as Repository
    const partial = initial.withLineSelection(1, true)
    selections.recordLineSelection(repository, file, partial)
    expect(selections.getBasis(partial)?.displayedDiff).toBe(diff)
    expect(
      selections.getBasis(partial)?.displayedDiff?.hunks[0].lines[2]
        .noTrailingNewLine
    ).toBe(true)
    const derived = partial.withLineSelection(2, true)
    selections.inheritDisplaySelection(
      derived,
      undefined,
      selections.getBasis(partial)
    )
    expect(selections.getBasis(derived)?.displayedDiff).toBe(diff)
    const recovery = vi
      .spyOn(selections, 'getRecoveryDiff')
      .mockResolvedValue(diff)
    const run = {
      repository,
      request: { files: [file.withSelection(partial)] },
      initialState: {
        branchesState: { tip: { kind: TipState.Unborn, ref: 'main' } },
      },
      inputVersions: ['verified-version'],
    } as never
    await selections.certifyUncaptured(run)
    expect(selections.getRestored(partial)?.displayedDiff).toBe(diff)
    expect(
      (
        await selections.verifyRestored(repository, [
          file.withSelection(partial),
        ])
      ).get(file.id)
    ).toBe(diff)
    const originalHunk = diff.hunks[0]
    const changedHunk = new DiffHunk(
      originalHunk.header,
      [
        diff.hunks[0].lines[0],
        diff.hunks[0].lines[1],
        diff.hunks[0].lines[2].withNoTrailingNewLine(false),
      ],
      originalHunk.unifiedDiffStart,
      originalHunk.unifiedDiffEnd,
      originalHunk.expansionType
    )
    expect(sameSelectionDiff(diff, { ...diff, hunks: [changedHunk] })).toBe(
      false
    )
    recovery.mockResolvedValue({ ...diff, hunks: [changedHunk] })
    await expect(selections.certifyUncaptured(run)).rejects.toThrow()
    await expect(
      selections.verifyRestored(repository, [file.withSelection(partial)])
    ).rejects.toThrow()
  })
})
