import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  hasRemotes: vi.fn(),
  remoteUrl: vi.fn(),
  manage: vi.fn(),
  refreshHead: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, read: () => Promise<unknown>) => read()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositorySafeFetchRemotes: mocks.hasRemotes,
    repositorySafeRemoteUrl: mocks.remoteUrl,
    manageRepositoryRemote: mocks.manage,
    refreshRepositoryRemoteHead: mocks.refreshHead,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/refs', () => ({ getSymbolicRef: vi.fn() }))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: vi.fn(),
}))

import {
  addRemote,
  getRemotes,
  removeRemote,
  setRemoteURL,
  updateRemoteHEAD,
} from '../../src/lib/git/remote'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo' } as Repository

describe('Rust remote discovery', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it.each(['ws', 'wry'])(
    'refreshes remote HEAD over %s without legacy Git',
    async transport => {
      mocks.transport = transport
      const remote = { name: '--test', url: '/remote' }
      await updateRemoteHEAD(repository, remote, true)
      expect(mocks.refreshHead).toHaveBeenCalledWith(
        '/repo',
        '--test',
        '/remote',
        true
      )
      expect(mocks.git).not.toHaveBeenCalled()
      mocks.refreshHead.mockRejectedValueOnce(new Error('outcome uncertain'))
      await expect(updateRemoteHEAD(repository, remote, false)).rejects.toThrow(
        'outcome uncertain'
      )
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('rejects unsupported remote HEAD reviews without legacy Git', async () => {
    mocks.transport = 'wry'
    mocks.refreshHead.mockResolvedValueOnce('unsupported')
    await expect(
      updateRemoteHEAD(repository, { name: 'origin', url: '/remote' }, false)
    ).rejects.toThrow('Remote HEAD cannot be reviewed')
    expect(mocks.git).not.toHaveBeenCalled()
    mocks.refreshHead.mockRejectedValueOnce(new Error('review changed'))
    await expect(
      updateRemoteHEAD(repository, { name: 'origin', url: '/remote' }, false)
    ).rejects.toThrow('review changed')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it.each(['ws', 'wry'])(
    'skips the legacy Git process when %s finds no remotes',
    async transport => {
      mocks.transport = transport
      mocks.hasRemotes.mockResolvedValueOnce([])
      await expect(getRemotes(repository)).resolves.toEqual([])
      expect(mocks.hasRemotes).toHaveBeenCalledWith('/repo')
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('returns ordinary configured remotes without invoking legacy Git', async () => {
    mocks.hasRemotes.mockResolvedValueOnce([
      { name: 'origin', url: 'https://example.test/repo' },
    ])
    await expect(getRemotes(repository)).resolves.toEqual([
      { name: 'origin', url: 'https://example.test/repo' },
    ])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('discovers remotes from the requested nested directory', async () => {
    mocks.hasRemotes.mockResolvedValueOnce([
      { name: 'origin', url: 'https://example.test/repo' },
    ])
    const nested = { path: '/repo/nested' } as Repository
    await expect(getRemotes(nested)).resolves.toEqual([
      { name: 'origin', url: 'https://example.test/repo' },
    ])
    expect(mocks.hasRemotes).toHaveBeenCalledWith('/repo/nested')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('rejects remote configuration that Rust cannot safely return', async () => {
    mocks.hasRemotes.mockResolvedValueOnce(null)
    await expect(getRemotes(repository)).rejects.toThrow(
      'cannot be safely represented'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates Rust errors rather than assuming the repository has no remotes', async () => {
    mocks.hasRemotes.mockRejectedValueOnce(
      new Error('configuration unavailable')
    )
    await expect(getRemotes(repository)).rejects.toThrow(
      'configuration unavailable'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it.each(['ws', 'wry'])(
    'manages supported remotes through %s under a mutation fence',
    async transport => {
      mocks.transport = transport
      mocks.manage.mockResolvedValue('updated')
      await expect(
        addRemote(repository, 'origin', 'https://example.test/repo')
      ).resolves.toEqual({ name: 'origin', url: 'https://example.test/repo' })
      await expect(
        setRemoteURL(repository, 'origin', 'https://example.test/new')
      ).resolves.toBe(true)
      await expect(removeRemote(repository, 'origin')).resolves.toBeUndefined()
      expect(mocks.manage.mock.calls).toEqual([
        [
          '/repo',
          { type: 'add', name: 'origin', url: 'https://example.test/repo' },
        ],
        [
          '/repo',
          { type: 'set_url', name: 'origin', url: 'https://example.test/new' },
        ],
        ['/repo', { type: 'remove', name: 'origin' }],
      ])
      expect(mocks.scope).toHaveBeenCalledTimes(3)
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('rejects unsupported remote URLs instead of bypassing review', async () => {
    mocks.manage.mockResolvedValueOnce('unsupported')
    await expect(
      setRemoteURL(
        repository,
        'origin',
        'https://user:secret@example.test/repo'
      )
    ).rejects.toThrow('unsupported by the reviewed native backend')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('does not retry a failed or fenced remote mutation', async () => {
    mocks.manage.mockRejectedValueOnce(new Error('update uncertain'))
    await expect(removeRemote(repository, 'origin')).rejects.toThrow(
      'update uncertain'
    )
    mocks.scope.mockRejectedValueOnce(new Error('repository locked'))
    await expect(removeRemote(repository, 'origin')).rejects.toThrow(
      'repository locked'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
