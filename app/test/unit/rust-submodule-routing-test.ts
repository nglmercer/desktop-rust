import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  status: vi.fn(),
  reset: vi.fn(),
  update: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositorySubmoduleStatus: mocks.status,
    repositoryResetSubmodules: mocks.reset,
    repositoryUpdateSubmodules: mocks.update,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: vi.fn(),
  getFallbackUrlForProxyResolve: vi.fn(),
}))
vi.mock('../../src/lib/git/authentication', () => ({
  AuthenticationErrors: new Set(),
}))
import {
  listSubmodules,
  resetSubmodulePaths,
  updateSubmodulesAfterOperation,
} from '../../src/lib/git/submodule'

describe.each(['wry', 'ws'])('Rust submodule status via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })
  it('parses initialized and changed entries and propagates errors', async () => {
    const sha = 'a'.repeat(40)
    const repository = { path: '/repo' } as Repository
    mocks.status
      .mockResolvedValueOnce(
        ` ${sha} folder with spaces (v1.0)\n+${sha} other (heads/main)\n-${sha} uninitialized\n`
      )
      .mockRejectedValueOnce(new Error('status failed'))
    expect(await listSubmodules(repository)).toMatchObject([
      { sha, path: 'folder with spaces', describe: 'v1.0' },
      { sha, path: 'other', describe: 'heads/main' },
    ])
    await expect(listSubmodules(repository)).rejects.toThrow('status failed')
    expect(mocks.status).toHaveBeenCalledWith('/repo')
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 2 : 0)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('resets only selected literal paths in the mutation scope', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.reset
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('reset failed'))
    await resetSubmodulePaths(repository, [])
    expect(mocks.reset).not.toHaveBeenCalled()
    await resetSubmodulePaths(repository, ['*-module'])
    expect(mocks.reset).toHaveBeenCalledWith('/repo', ['*-module'])
    await expect(resetSubmodulePaths(repository, ['*-module'])).rejects.toThrow(
      'reset failed'
    )
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('updates submodules with explicit protocol permission and progress', async () => {
    const repository = { path: '/repo' } as Repository
    const progress = vi.fn()
    mocks.update
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('update failed'))
    await updateSubmodulesAfterOperation(
      repository,
      null,
      progress,
      'checkout',
      'Checkout',
      'main',
      true
    )
    expect(mocks.update).toHaveBeenCalledWith('/repo', true)
    expect(progress).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({ value: 0, target: 'main' })
    )
    expect(progress).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ value: 1, target: 'main' })
    )
    await expect(
      updateSubmodulesAfterOperation(
        repository,
        null,
        progress,
        'checkout',
        'Checkout',
        'main',
        false
      )
    ).rejects.toThrow('update failed')
    expect(progress).toHaveBeenCalledTimes(3)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
