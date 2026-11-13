import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  fetch: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, run: () => Promise<unknown>) => run()),
  environment: vi.fn(async () => ({})),
  proxy: vi.fn(async (): Promise<Record<string, string> | undefined> => undefined),
  convert: vi.fn(() => new Error('classified fetch failure')),
  PreviewUnsupported: class extends Error {},
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ transportKind: mocks.transport, fetchRepositoryRemote: mocks.fetch }),
  FetchPreviewUnsupported: mocks.PreviewUnsupported,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/environment', () => ({ envForRemoteOperation: mocks.environment, envForProxy: mocks.proxy }))
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
    for (const key of ['http_proxy', 'https_proxy', 'HTTPS_PROXY', 'all_proxy', 'ALL_PROXY']) vi.stubEnv(key, undefined)
    mocks.transport = 'ws'
    mocks.fetch.mockReset()
    mocks.proxy.mockReset().mockResolvedValue(undefined)
    mocks.git.mockResolvedValue(undefined)
  })
  afterEach(() => {
    vi.unstubAllEnvs()
    Reflect.deleteProperty(window, '__desktop')
  })

  it.each(['ws', 'wry'])('routes %s fetch with recursion, background, owner, and streamed progress', async transport => {
    mocks.transport = transport
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invokeWithProgress: vi.fn() } })
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

  it.each(['ws', 'wry'])('converts completed failures and never retries uncertain %s outcomes via Git', async transport => {
    mocks.transport = transport
    const failure = { exitCode: 128, errorKind: 'https_authentication_failed' }
    mocks.fetch.mockResolvedValueOnce(failure).mockRejectedValueOnce(new Error('outcome uncertain'))
    await expect(fetch(repository, remote)).rejects.toThrow('classified fetch failure')
    expect(mocks.convert).toHaveBeenCalledWith(failure, '/repo', 'origin')
    await expect(fetch(repository, remote)).rejects.toThrow('outcome uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it.each(['ws', 'wry'])('fetches an explicit refspec via %s and retains exit 128 handling', async transport => {
    mocks.transport = transport
    const refspec = '+refs/heads/main:refs/remotes/origin/review'
    mocks.fetch.mockResolvedValueOnce(undefined).mockResolvedValueOnce({ exitCode: 128, errorKind: null }).mockResolvedValueOnce({ exitCode: 1, errorKind: null }).mockRejectedValueOnce(new Error('uncertain'))
    await fetchRefspec(repository, remote, refspec)
    expect(mocks.fetch).toHaveBeenCalledWith('/repo', 'origin', remote.url, true, false, 'example', undefined, refspec)
    await expect(fetchRefspec(repository, remote, '^refs/heads/private')).resolves.toBeUndefined()
    await expect(fetchRefspec(repository, remote, refspec)).rejects.toThrow('classified fetch failure')
    await expect(fetchRefspec(repository, remote, refspec)).rejects.toThrow('uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('keeps the existing Electron Git path for ordinary fetch', async () => {
    mocks.transport = 'none'
    await fetch(repository, remote, undefined, false, false)
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledWith(['fetch', '--prune', '--recurse-submodules=no', '--', 'origin'], '/repo', 'fetch', expect.any(Object))
  })

  it('falls back from ordinary Wry fetch only for unsupported pre-mutation review', async () => {
    mocks.transport = 'wry'
    mocks.fetch.mockRejectedValueOnce(new mocks.PreviewUnsupported())
    await fetch(repository, remote, undefined, false, false)
    expect(mocks.git).toHaveBeenCalledWith(['fetch', '--prune', '--recurse-submodules=no', '--', 'origin'], '/repo', 'fetch', expect.any(Object))
    mocks.git.mockClear()
    mocks.fetch.mockRejectedValueOnce(new Error('outcome uncertain'))
    await expect(fetch(repository, remote)).rejects.toThrow('outcome uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses legacy ordinary Wry fetch if a proxy or older progress host requires it', async () => {
    mocks.transport = 'wry'
    mocks.proxy.mockResolvedValueOnce({ https_proxy: 'http://proxy.invalid:8080' })
    await fetch(repository, remote)
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledTimes(1)
    mocks.git.mockClear()
    Object.defineProperty(window, '__desktop', { configurable: true, value: {} })
    await fetch(repository, remote, vi.fn())
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledTimes(1)
  })

  it('uses the legacy refspec path without a backend', async () => {
    mocks.transport = 'none'
    await fetchRefspec(repository, remote, 'refs/heads/main')
    expect(mocks.git).toHaveBeenCalledWith(['fetch', '--', 'origin', 'refs/heads/main'], '/repo', 'fetchRefspec', expect.objectContaining({ successExitCodes: new Set([0, 128]) }))
  })

  it('uses the Wry legacy path only when the pre-mutation URL review is unsupported', async () => {
    mocks.transport = 'wry'
    mocks.fetch.mockRejectedValueOnce(new mocks.PreviewUnsupported())
    await fetchRefspec(repository, remote, 'refs/heads/main')
    expect(mocks.git).toHaveBeenCalledTimes(1)
    mocks.git.mockClear()
    mocks.fetch.mockRejectedValueOnce(new Error('outcome uncertain'))
    await expect(fetchRefspec(repository, remote, 'refs/heads/main')).rejects.toThrow('outcome uncertain')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses the Wry legacy path when an OS or PAC proxy must be injected', async () => {
    mocks.transport = 'wry'
    mocks.proxy.mockResolvedValue({ https_proxy: 'http://proxy.invalid:8080' })
    await fetchRefspec(repository, remote, 'refs/heads/main')
    expect(mocks.proxy).toHaveBeenCalledWith(remote.url)
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.environment).toHaveBeenCalledWith(remote.url)
    expect(mocks.git).toHaveBeenCalledTimes(1)
  })

  it('keeps inherited proxy settings on the existing Wry Git path', async () => {
    mocks.transport = 'wry'
    vi.stubEnv('HTTPS_PROXY', 'http://proxy.invalid:8080')
    await fetchRefspec(repository, remote, 'refs/heads/main')
    expect(mocks.fetch).not.toHaveBeenCalled()
    expect(mocks.environment).toHaveBeenCalledWith(remote.url)
    expect(mocks.git).toHaveBeenCalledTimes(1)
  })
})
