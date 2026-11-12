import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffSelection,
  DiffSelectionType,
  DiffType,
} from '../../src/models/diff'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  diff: vi.fn(),
  discard: vi.fn(),
  batchPreview: vi.fn(),
  batch: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    repositoryWorkingDiff: mocks.diff,
    repositoryDiscardSelection: mocks.discard,
    repositoryDiscardBatchPreview: mocks.batchPreview,
    repositoryDiscardBatch: mocks.batch,
  }),
}))
import {
  discardBatchWithoutDialog,
  discardReviewedSelection,
} from '../../src/lib/git/reviewed-discard'

const raw = new DiffParser().parse(`diff --git a/file b/file
--- a/file
+++ b/file
@@ -1 +1 @@
-old
+new
`)
const displayed = {
  kind: DiffType.Text as const,
  text: raw.contents,
  hunks: raw.hunks,
  maxLineNumber: raw.maxLineNumber,
  hasHiddenBidiChars: false,
}
const selection = DiffSelection.fromInitialSelection(
  DiffSelectionType.None
).withLineSelection(2, true)
const repository = { path: '/repo' } as Repository
const preview = {
  path: 'file',
  basis: 'diff-basis',
  kind: 'text',
  can_select: true,
  hunks: [
    {
      old_start: 1,
      old_count: 1,
      new_start: 1,
      new_count: 1,
      lines: [
        {
          id: 0,
          kind: 'deleted',
          text: 'old',
          old_line: 1,
          new_line: null,
          no_newline: false,
        },
        {
          id: 1,
          kind: 'added',
          text: 'new',
          old_line: null,
          new_line: 1,
          no_newline: false,
        },
      ],
    },
  ],
}

describe('reviewed selected-line discard', () => {
  beforeEach(() => {
    vi.resetAllMocks()
    mocks.diff.mockResolvedValue(preview)
    mocks.discard.mockResolvedValue('undo-id')
  })
  it('sends the certified selected lines for durable discard', async () => {
    await expect(
      discardReviewedSelection(repository, 'file', displayed, selection)
    ).resolves.toBe('undo-id')
    expect(mocks.discard).toHaveBeenCalledWith('/repo', 'file', {
      basis: 'diff-basis',
      lines: [1],
    })
  })
  it('does not mutate when the displayed diff is stale', async () => {
    mocks.diff.mockResolvedValue({
      ...preview,
      hunks: [
        {
          ...preview.hunks[0],
          lines: [
            preview.hunks[0].lines[0],
            { ...preview.hunks[0].lines[1], text: 'changed' },
          ],
        },
      ],
    })
    await expect(
      discardReviewedSelection(repository, 'file', displayed, selection)
    ).rejects.toThrow('selected diff changed')
    expect(mocks.discard).not.toHaveBeenCalled()
  })
  it('does nothing for an empty selection', async () => {
    await expect(
      discardReviewedSelection(
        repository,
        'file',
        displayed,
        DiffSelection.fromInitialSelection(DiffSelectionType.None)
      )
    ).resolves.toBeNull()
    expect(mocks.diff).not.toHaveBeenCalled()
  })
})

describe('batch discard without a confirmation dialog', () => {
  beforeEach(() => vi.resetAllMocks())
  it('rejects additional affected paths rather than silently discarding them', async () => {
    mocks.batchPreview.mockResolvedValue({
      basis: 'review',
      files: ['new', 'old'],
    })
    await expect(
      discardBatchWithoutDialog(repository, ['new'])
    ).rejects.toThrow('Additional files are affected')
    expect(mocks.batch).not.toHaveBeenCalled()
  })
  it('submits the certified basis when only selected paths are affected', async () => {
    mocks.batchPreview.mockResolvedValue({ basis: 'review', files: ['new'] })
    mocks.batch.mockResolvedValue('undo')
    await expect(discardBatchWithoutDialog(repository, ['new'])).resolves.toBe(
      'undo'
    )
    expect(mocks.batch).toHaveBeenCalledWith('/repo', ['new'], 'review')
  })
})
