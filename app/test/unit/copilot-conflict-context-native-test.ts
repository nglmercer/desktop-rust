import { createHash } from 'node:crypto'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const native = vi.hoisted(() => ({
  transportKind: 'ws',
  repositoryReadSnapshot: vi.fn(),
  repositoryCopilotConflictBasis: vi.fn(),
  readLease: vi.fn(
    async (_path: string, _kind: string, read: () => Promise<unknown>) => read()
  ),
  BackendError: class extends Error {
    constructor(public code: string) {
      super(code)
    }
  },
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => native,
  BackendError: native.BackendError,
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: native.readLease,
}))
vi.mock('../../src/lib/git/merge', () => ({ getMergeBase: vi.fn() }))
vi.mock('../../src/lib/git/log', () => ({ getCommits: vi.fn() }))

import { buildConflictContext } from '../../src/lib/copilot-conflict-context'

const content =
  '\ufeffheader\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n'
const bytes = new TextEncoder().encode(content)
const certificate = {
  basis: 'a'.repeat(64),
  sha256: createHash('sha256').update(bytes).digest('hex'),
  size: bytes.length,
  ours: true,
  theirs: true,
}

beforeEach(() => {
  native.repositoryReadSnapshot.mockResolvedValue(bytes)
  native.repositoryCopilotConflictBasis.mockImplementation(
    async (_repository: string, path: string) =>
      path === 'deleted.txt'
        ? { ...certificate, sha256: null, size: null, ours: false }
        : certificate
  )
})
afterEach(() => {
  native.repositoryReadSnapshot.mockReset()
  native.repositoryCopilotConflictBasis.mockReset()
  native.readLease.mockClear()
  native.transportKind = 'ws'
})

describe('native Copilot conflict context', () => {
  for (const transport of ['ws', 'wry']) {
    it(`captures and verifies bounded conflict content through ${transport}`, async () => {
      native.transportKind = transport
      const result = await buildConflictContext('ours', 'theirs', '/repo', [
        { path: 'conflict.txt' },
        { path: 'deleted.txt', deletedSide: 'ours' },
      ])
      expect(native.repositoryReadSnapshot).toHaveBeenCalledOnce()
      expect(native.repositoryReadSnapshot).toHaveBeenCalledWith(
        '/repo',
        'renderer_working_file',
        {
          path: 'conflict.txt',
          max_bytes: 10_485_760,
        },
        certificate.sha256
      )
      expect(native.repositoryCopilotConflictBasis).toHaveBeenCalledTimes(4)
      expect(native.readLease).toHaveBeenCalledTimes(
        transport === 'wry' ? 2 : 0
      )
      expect(result.files[0].rawContent).toBe(content)
      expect(result.files[0].conflictBasis).toBe(certificate.basis)
      expect(result.files[0].hunks[0].oursContent).toBe('ours')
      expect(result.files[1].deleteConflict).toEqual({ deletedSide: 'ours' })
      expect(result.files[1].conflictBasis).toBe(certificate.basis)
    })
  }

  it('skips unreadable files without falling back to Node', async () => {
    native.repositoryReadSnapshot.mockRejectedValue(new Error('unsafe path'))
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: '../outside' },
    ])
    expect(result.files[0]).toEqual({
      path: '../outside',
      hunks: [],
      skippedReason: 'Conflict changed or could not be read',
    })
  })

  it('reports when a conflict file exceeds the native read limit', async () => {
    native.repositoryReadSnapshot.mockRejectedValue(
      new native.BackendError('file_too_large')
    )
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: 'large.txt' },
    ])
    expect(result.files[0].skippedReason).toBe(
      'File too large to resolve automatically'
    )
  })

  it('skips content when the native snapshot rejects a digest mismatch', async () => {
    native.repositoryReadSnapshot.mockRejectedValue(
      new native.BackendError('conflict_changed')
    )
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: 'conflict.txt' },
    ])
    expect(result.files[0]).toEqual({
      path: 'conflict.txt',
      hunks: [],
      skippedReason: 'Conflict changed or could not be read',
    })
  })

  it('refuses a conflict whose certificate changes during the read', async () => {
    native.repositoryCopilotConflictBasis
      .mockResolvedValueOnce(certificate)
      .mockResolvedValueOnce({ ...certificate, basis: 'b'.repeat(64) })
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: 'conflict.txt' },
    ])
    expect(result.files[0]).toEqual({
      path: 'conflict.txt',
      hunks: [],
      skippedReason: 'Conflict changed or could not be read',
    })
  })

  it('refuses mismatched delete-vs-modify sides', async () => {
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: 'deleted.txt', deletedSide: 'theirs' },
    ])
    expect(result.files[0]).toEqual({
      path: 'deleted.txt',
      hunks: [],
      skippedReason: 'Conflict changed or could not be read',
    })
    expect(native.repositoryReadSnapshot).not.toHaveBeenCalled()
  })

  it('keeps large delete-vs-modify conflicts available without reading content', async () => {
    native.repositoryCopilotConflictBasis.mockResolvedValue({
      ...certificate,
      size: 10_485_761,
      ours: true,
      theirs: false,
    })
    const result = await buildConflictContext('ours', 'theirs', '/repo', [
      { path: 'large-delete', deletedSide: 'theirs' },
    ])
    expect(result.files[0].deleteConflict).toEqual({ deletedSide: 'theirs' })
    expect(result.files[0].conflictBasis).toBe(certificate.basis)
    expect(native.repositoryReadSnapshot).not.toHaveBeenCalled()
    expect(native.repositoryCopilotConflictBasis).toHaveBeenCalledTimes(2)
    expect(native.repositoryCopilotConflictBasis).toHaveBeenCalledWith(
      '/repo',
      'large-delete',
      null,
      true
    )
  })

  it('bounds concurrent conflict reviews while preserving file order', async () => {
    let active = 0
    let maximum = 0
    native.repositoryCopilotConflictBasis.mockImplementation(async () => {
      active++
      maximum = Math.max(maximum, active)
      await new Promise(resolve => setTimeout(resolve, 1))
      active--
      return certificate
    })
    const files = Array.from({ length: 40 }, (_, index) => ({
      path: `file-${index}`,
    }))
    const result = await buildConflictContext('ours', 'theirs', '/repo', files)
    expect(maximum).toBeLessThanOrEqual(4)
    expect(result.files.map(file => file.path)).toEqual(
      files.map(file => file.path)
    )
    expect(
      result.files.every(file => file.conflictBasis === certificate.basis)
    ).toBe(true)
  })
})
