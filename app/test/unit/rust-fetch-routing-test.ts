import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  fetch: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, run: () => Promise<unknown>) => run()),
  environment: vi.fn(async () => ({})),
  convert: vi.fn(() => new Error('classified fetch failure')),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ transportKind: mocks.transport, fetchRepositoryRemote: mocks.fetch }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/environment', () => ({ envForRemoteOperation: mocks.environment }))
vi.mock('../../src/lib/git/fetch-error', () => ({ fetchFailureToGitError: mocks.convert }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))

import { fetch, fetchRefspec } from '../../src/lib/git/fetch'

const repository = {
  path: '/repo',
  gitHubRepository: { owner: { login: 'example' } },
} as Repository
const remote = { name: 'origin', url: 'https://example.invalid/owner/repo' }

describe('Rust fetch routing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
    mocks.fetch.mockReset()
    mocks.git.mockResolvedValue(undefined)
  })

  it('routes WebSocket fetch with recursion, background, owner, and streamed progress', async () => {
    mocks.fetch.mockImplementation(async (_path, _remote, _url, _recurse, _background, _owner, onProgress) => {
      onProgress({ stream: 'stderr', data: btoa('Receiving objects: 50% (1/2)\r') })
    })
    const progress = vi.fn()
    await fetch(repository, remote, progress, true, false)
    expect(mocks.scope).toHaveBeenCalledWith('/repo', 'mutation', expect.any(Function), true)
    expect(mocks.fetch).toHaveBeenCalledWith('/repo', 'origin', remote.url, false, true, 'example', expect.any(Function))
    expect(progress).toHaveBeenCalledWith(expect.objectContaining({ kind: 'fetch', title: 'Fetching origin', value: 0, remote: 'origin' }))
    expect(progress).toHaveBeenCalledWith(expect.objectContaining({ description: expect.stringContaining('Receiving objects'), remote: 'origin' }))
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('converts completed failures and never retries uncertain outcomes via Git', async () => {
    const failure = { exitCode: 128, errorKind: 'https_authentication_failed' }
    mocks.fetch.mockResolvedValueOnce(failure).mockRejectedValueOnce(new Error('outcome uncertain'))
    await expect(fetch(repository, remote)).rejects.toThrow('classified fetch failure')
    expect(mocks.convert).toHaveBeenCalledWith(failure, '/repo', 'origin')
    await expect(fetch(repository, remote)).rejects.toThrow('outcome uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('fetches an explicit refspec and retains exit 128 handling', async () => {
    const refspec = '+refs/heads/main:refs/remotes/origin/review'
    mocks.fetch.mockResolvedValueOnce(undefined).mockResolvedValueOnce({ exitCode: 128, errorKind: null }).mockResolvedValueOnce({ exitCode: 1, errorKind: null }).mockRejectedValueOnce(new Error('uncertain'))
    await fetchRefspec(repository, remote, refspec)
    expect(mocks.fetch).toHaveBeenCalledWith('/repo', 'origin', remote.url, true, false, 'example', undefined, refspec)
    await expect(fetchRefspec(repository, remote, '^refs/heads/private')).resolves.toBeUndefined()
    await expect(fetchRefspec(repository, remote, refspec)).rejects.toThrow('classified fetch failure')
    await expect(fetchRefspec(repository, remote, refspec)).rejects.toThrow('uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it.each(['wry', 'none'])('keeps the existing %s Git path', async transport => {
    mocks.transport = transport
    await fetch(repository, remote, undefined, false, false)
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledWith(['fetch', '--prune', '--recurse-submodules=no', '--', 'origin'], '/repo', 'fetch', expect.any(Object))
    await fetchRefspec(repository, remote, 'refs/heads/main')
    expect(mocks.git).toHaveBeenCalledWith(['fetch', '--', 'origin', 'refs/heads/main'], '/repo', 'fetchRefspec', expect.objectContaining({ successExitCodes: new Set([0, 128]) }))
  })
})
