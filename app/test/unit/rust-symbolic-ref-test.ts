import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  read: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, read: () => Promise<unknown>) => read()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositorySymbolicRef: mocks.read,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))

import { getSymbolicRef } from '../../src/lib/git/refs'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo' } as Repository

describe('symbolic refs', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it.each(['ws', 'wry'])(
    'reads the requested ref through %s',
    async transport => {
      mocks.transport = transport
      mocks.read
        .mockResolvedValueOnce('refs/heads/main')
        .mockResolvedValueOnce(null)
      await expect(getSymbolicRef(repository, 'HEAD')).resolves.toBe(
        'refs/heads/main'
      )
      await expect(
        getSymbolicRef(repository, 'refs/remotes/origin/HEAD')
      ).resolves.toBeNull()
      expect(mocks.read.mock.calls).toEqual([
        ['/repo', 'HEAD'],
        ['/repo', 'refs/remotes/origin/HEAD'],
      ])
      expect(mocks.git).not.toHaveBeenCalled()
      if (transport === 'wry')
        expect(mocks.scope).toHaveBeenCalledWith(
          '/repo',
          'read',
          expect.any(Function),
          true
        )
    }
  )

  it('propagates backend failures without retrying through the legacy process', async () => {
    mocks.read.mockRejectedValueOnce(new Error('disconnected'))
    await expect(getSymbolicRef(repository, 'HEAD')).rejects.toThrow(
      'disconnected'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('honors the native repository operation fence', async () => {
    mocks.transport = 'wry'
    mocks.scope.mockRejectedValueOnce(new Error('repository locked'))
    await expect(getSymbolicRef(repository, 'HEAD')).rejects.toThrow(
      'repository locked'
    )
    expect(mocks.read).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves the Electron path', async () => {
    mocks.transport = 'none'
    mocks.git.mockResolvedValueOnce({
      exitCode: 0,
      stdout: 'refs/heads/main\n',
    })
    await expect(getSymbolicRef(repository, 'HEAD')).resolves.toBe(
      'refs/heads/main'
    )
    expect(mocks.read).not.toHaveBeenCalled()
  })
})
