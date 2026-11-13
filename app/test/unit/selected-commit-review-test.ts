import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffSelection,
  DiffSelectionType,
  DiffType,
  type ITextDiff,
} from '../../src/models/diff'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'

const calls = vi.hoisted(() => ({ diff: vi.fn(), review: vi.fn() }))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    copilotCommitDiffForRepository: calls.diff,
    repositorySelectedCommitPreview: calls.review,
  }),
}))
import { reviewSelectedCommit } from '../../src/lib/git/selected-commit-review'

const repository = { path: '/repo' } as Repository
const all = DiffSelection.fromInitialSelection(DiffSelectionType.All)
const none = DiffSelection.fromInitialSelection(DiffSelectionType.None)
const raw = new DiffParser().parse(
  'diff --git a/old b/new\n--- a/old\n+++ b/new\n@@ -1 +1 @@\n-old\n+new\n'
)
const displayed: ITextDiff = {
  kind: DiffType.Text,
  text: raw.contents,
  hunks: raw.hunks,
  maxLineNumber: raw.maxLineNumber,
  hasHiddenBidiChars: false,
}
const partial = new WorkingDirectoryFileChange(
  'new',
  {
    kind: AppFileStatusKind.Renamed,
    oldPath: 'old',
    renameIncludesModifications: true,
  },
  none.withLineSelection(1, true)
)
const fresh = {
  path: 'new',
  basis: 'basis',
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

describe('selected commit review', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    calls.diff.mockResolvedValue(fresh)
    calls.review.mockResolvedValue({ basis: 'reviewed' })
  })

  it('reviews whole, deleted, and partial renamed files with the retained head', async () => {
    const files = [
      partial,
      new WorkingDirectoryFileChange(
        'added',
        { kind: AppFileStatusKind.Untracked },
        all
      ),
      new WorkingDirectoryFileChange(
        'deleted',
        { kind: AppFileStatusKind.Deleted },
        all
      ),
    ]
    const head = { ref: 'refs/heads/main', sha: 'abc' }
    const reviewed = await reviewSelectedCommit(
      repository,
      files,
      () => displayed,
      'amend',
      head
    )
    expect(reviewed.basis).toBe('reviewed')
    expect(calls.diff).toHaveBeenCalledWith('/repo', 'new', {}, 'old')
    expect(calls.review).toHaveBeenCalledWith('/repo', {
      paths: ['added'],
      remove_paths: ['deleted'],
      partial_selections: [
        { path: 'new', original_path: 'old', basis: 'basis', lines: [0] },
      ],
      allow_empty: false,
      amend_head: 'amend',
      expected_head: head,
    })
    expect(reviewed.selection).toEqual(calls.review.mock.calls[0][1])
  })

  it('rejects missing or changed displayed lines before requesting a review', async () => {
    await expect(
      reviewSelectedCommit(repository, [partial], () => undefined)
    ).rejects.toThrow('Review and reselect')
    calls.diff.mockResolvedValue({
      ...fresh,
      hunks: [
        {
          ...fresh.hunks[0],
          lines: [
            { ...fresh.hunks[0].lines[0], text: 'changed' },
            fresh.hunks[0].lines[1],
          ],
        },
      ],
    })
    await expect(
      reviewSelectedCommit(repository, [partial], () => displayed)
    ).rejects.toThrow('Review and reselect')
    expect(calls.review).not.toHaveBeenCalled()
  })
  it('preserves explicit permission for an empty commit', async () => {
    await reviewSelectedCommit(
      repository,
      [],
      () => undefined,
      undefined,
      undefined,
      true
    )
    expect(calls.review).toHaveBeenCalledWith(
      '/repo',
      expect.objectContaining({
        paths: [],
        partial_selections: [],
        allow_empty: true,
      })
    )
  })
})
