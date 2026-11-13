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
  getRemoteURL,
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

  it.each(['ws', 'wry'])('refreshes remote HEAD over %s without legacy Git', async transport => {
    mocks.transport = transport
    const remote = { name: '--test', url: '/remote' }
    await updateRemoteHEAD(repository, remote, true)
    expect(mocks.refreshHead).toHaveBeenCalledWith('/repo', '--test', '/remote', true)
    expect(mocks.git).not.toHaveBeenCalled()
    mocks.refreshHead.mockRejectedValueOnce(new Error('outcome uncertain'))
    await expect(updateRemoteHEAD(repository, remote, false)).rejects.toThrow('outcome uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses legacy Git only after a Wry preview reports unsupported', async () => {
    mocks.transport = 'wry'
    mocks.refreshHead.mockResolvedValueOnce('unsupported')
    await updateRemoteHEAD(repository, { name: 'origin', url: '/remote' }, false)
    expect(mocks.git).toHaveBeenCalledTimes(1)
    expect(mocks.git.mock.calls[0][0]).toEqual(['remote', 'set-head', '-a', '--', 'origin'])
    mocks.git.mockClear()
    mocks.refreshHead.mockRejectedValueOnce(new Error('review changed'))
    await expect(updateRemoteHEAD(repository, { name: 'origin', url: '/remote' }, false)).rejects.toThrow('review changed')
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

  it('preserves the existing remote URL when Rust cannot safely return it', async () => {
    mocks.hasRemotes.mockResolvedValueOnce(null)
    mocks.git
      .mockResolvedValueOnce({ exitCode: 0, stdout: '.git\n' })
      .mockResolvedValueOnce({
        exitCode: 0,
        stdout: 'remote.origin.url\nhttps://user:secret@example.test/repo\0',
      })
      .mockResolvedValueOnce({
        exitCode: 0,
        stdout: 'https://user:secret@example.test/repo\n',
      })
    await expect(getRemotes(repository)).resolves.toEqual([
      { name: 'origin', url: 'https://user:secret@example.test/repo' },
    ])
    expect(mocks.git).toHaveBeenCalledTimes(3)
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

  it('does not use the shortcut for explicit Git environment overrides', async () => {
    mocks.git
      .mockResolvedValueOnce({ exitCode: 0, stdout: '.git\n' })
      .mockResolvedValueOnce({ exitCode: 1, stdout: '' })
    await expect(
      getRemotes(repository, { env: { GIT_CONFIG_GLOBAL: '/custom' } })
    ).resolves.toEqual([])
    expect(mocks.hasRemotes).not.toHaveBeenCalled()
  })

  it.each(['ws', 'wry'])(
    'reads a named remote on %s and preserves missing results',
    async transport => {
      mocks.transport = transport
      mocks.remoteUrl
        .mockResolvedValueOnce({
          kind: 'resolved',
          url: 'https://example.test/repo',
        })
        .mockResolvedValueOnce({ kind: 'missing' })
      await expect(getRemoteURL(repository, 'origin')).resolves.toBe(
        'https://example.test/repo'
      )
      await expect(getRemoteURL(repository, 'missing')).resolves.toBeNull()
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('retains credential-bearing remote URL behavior', async () => {
    mocks.remoteUrl.mockResolvedValueOnce({ kind: 'unsupported' })
    mocks.git.mockResolvedValueOnce({
      exitCode: 0,
      stdout: 'https://user:secret@example.test/repo\n',
    })
    await expect(getRemoteURL(repository, 'origin')).resolves.toBe(
      'https://user:secret@example.test/repo'
    )
    expect(mocks.git).toHaveBeenCalledTimes(1)
  })

  it('propagates named-remote backend failures', async () => {
    mocks.remoteUrl.mockRejectedValueOnce(new Error('Git config unavailable'))
    await expect(getRemoteURL(repository, 'origin')).rejects.toThrow(
      'Git config unavailable'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('honors the native repository operation fence for remote reads', async () => {
    mocks.transport = 'wry'
    mocks.scope.mockRejectedValueOnce(new Error('repository locked'))
    await expect(getRemoteURL(repository, 'origin')).rejects.toThrow(
      'repository locked'
    )
    expect(mocks.remoteUrl).not.toHaveBeenCalled()
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

  it('retains the existing path for unsupported remote URLs', async () => {
    mocks.manage.mockResolvedValueOnce('unsupported')
    mocks.git.mockResolvedValueOnce({ exitCode: 0 })
    await setRemoteURL(
      repository,
      'origin',
      'https://user:secret@example.test/repo'
    )
    expect(mocks.git).toHaveBeenCalledWith(
      [
        'remote',
        'set-url',
        '--',
        'origin',
        'https://user:secret@example.test/repo',
      ],
      '/repo',
      'setRemoteURL'
    )
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
