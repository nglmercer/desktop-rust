import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  snapshot: vi.fn(),
  legacyRead: vi.fn(),
  blob: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, action: () => Promise<unknown>) =>
      action()
  ),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryReadSnapshot: mocks.snapshot,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/file-system', () => ({
  readPartialFile: mocks.legacyRead,
}))
vi.mock('../../src/lib/git/show', () => ({
  getPartialBlobContents: mocks.blob,
}))
vi.mock('../../src/lib/highlighter/worker', () => ({ highlight: vi.fn() }))

import { getFileContents } from '../../src/ui/diff/syntax-highlighting'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'
import { DiffSelection, DiffSelectionType } from '../../src/models/diff'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repository' } as Repository
const file = new WorkingDirectoryFileChange(
  'nested/file.ts',
  { kind: AppFileStatusKind.Modified },
  DiffSelection.fromInitialSelection(DiffSelectionType.All)
)

describe('Rust working file highlighting', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
    mocks.snapshot.mockResolvedValue(
      new TextEncoder().encode('new first\nnew second\n')
    )
    mocks.blob.mockResolvedValue(Buffer.from('old first\n'))
  })

  for (const transport of ['ws', 'wry']) {
    it(`loads the working prefix through ${transport} without Node file reads`, async () => {
      mocks.transport = transport
      const result = await getFileContents(repository, file)
      expect(result.oldContents).toEqual(['old first', ''])
      expect(result.newContents).toEqual(['new first', 'new second', ''])
      expect(result.canBeExpanded).toBe(true)
      expect(mocks.snapshot).toHaveBeenCalledWith(
        '/repository',
        'renderer_working_file',
        {
          path: 'nested/file.ts',
          max_bytes: 1024 * 1024,
          prefix: true,
        }
      )
      expect(mocks.legacyRead).not.toHaveBeenCalled()
      expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
    })
  }

  it('does not expand a prefix that fills the limit', async () => {
    mocks.snapshot.mockResolvedValue(new Uint8Array(1024 * 1024).fill(97))
    const result = await getFileContents(repository, file)
    expect(result.newContents[0]).toHaveLength(1024 * 1024)
    expect(result.canBeExpanded).toBe(false)
  })

  it('treats a failed working read as unavailable without falling back to Node', async () => {
    mocks.snapshot.mockRejectedValue(new Error('inaccessible'))
    const result = await getFileContents(repository, file)
    expect(result.newContents).toEqual([])
    expect(result.canBeExpanded).toBe(false)
    expect(mocks.legacyRead).not.toHaveBeenCalled()
  })
})
