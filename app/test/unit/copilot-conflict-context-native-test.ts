import { afterEach, describe, expect, it, vi } from 'vitest'

const native = vi.hoisted(() => ({
  transportKind: 'ws',
  repositoryReadSnapshot: vi.fn(),
  readLease: vi.fn(async (_path: string, _kind: string, read: () => Promise<unknown>) => read()),
  BackendError: class extends Error { constructor(public code: string) { super(code) } },
}))
vi.mock('../../src/backend/client', () => ({ backend: () => native, BackendError: native.BackendError }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: native.readLease }))
vi.mock('../../src/lib/git/merge', () => ({ getMergeBase: vi.fn() }))
vi.mock('../../src/lib/git/log', () => ({ getCommits: vi.fn() }))

import { buildConflictContext } from '../../src/lib/copilot-conflict-context'

afterEach(() => {
  native.repositoryReadSnapshot.mockReset()
  native.readLease.mockClear()
  native.transportKind = 'ws'
})

describe('native Copilot conflict context', () => {
  for (const transport of ['ws', 'wry']) {
    it(`reads bounded conflict content through ${transport}`, async () => {
      native.transportKind = transport
      const content = '\ufeffheader\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n'
      native.repositoryReadSnapshot.mockResolvedValue(new TextEncoder().encode(content))
      const result = await buildConflictContext('ours', 'theirs', '/repo', [
        { path: 'conflict.txt' },
        { path: 'deleted.txt', deletedSide: 'ours' },
      ])
      expect(native.repositoryReadSnapshot).toHaveBeenCalledOnce()
      expect(native.repositoryReadSnapshot).toHaveBeenCalledWith('/repo', 'renderer_working_file', {
        path: 'conflict.txt', max_bytes: 10_485_760,
      })
      expect(native.readLease).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
      expect(result.files[0].rawContent).toBe(content)
      expect(result.files[0].hunks[0].oursContent).toBe('ours')
      expect(result.files[1].deleteConflict).toEqual({ deletedSide: 'ours' })
    })
  }

  it('skips unreadable files without falling back to Node', async () => {
    native.repositoryReadSnapshot.mockRejectedValue(new Error('unsafe path'))
    const result = await buildConflictContext('ours', 'theirs', '/repo', [{ path: '../outside' }])
    expect(result.files[0]).toEqual({ path: '../outside', hunks: [], skippedReason: 'File could not be read' })
  })

  it('reports when a conflict file exceeds the native read limit', async () => {
    native.repositoryReadSnapshot.mockRejectedValue(new native.BackendError('file_too_large'))
    const result = await buildConflictContext('ours', 'theirs', '/repo', [{ path: 'large.txt' }])
    expect(result.files[0].skippedReason).toBe('File too large to resolve automatically')
  })
})
