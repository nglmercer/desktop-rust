import { describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({ git: vi.fn(), progress: vi.fn() }))

vi.mock('../../src/lib/git/core', () => ({
  git: mocks.git,
  gitRebaseArguments: () => [],
}))
vi.mock('../../src/lib/git/environment', () => ({ envForRemoteOperation: async () => ({}) }))
vi.mock('../../src/lib/git/config', () => ({ getConfigValue: async () => null }))
vi.mock('../../src/lib/progress', () => ({
  PullProgressParser: class {},
  executionOptionsWithProgress: async (options: unknown) => {
    mocks.progress(options)
    return options
  },
}))

import { pull } from '../../src/lib/git/pull'

describe('pull hook callbacks', () => {
  it.each([false, true])('passes callbacks to Git with progress enabled: %s', async withProgress => {
    mocks.git.mockReset().mockResolvedValue(undefined)
    mocks.progress.mockClear()
    const onHookProgress = vi.fn()
    const onHookFailure = vi.fn(async () => 'abort' as const)
    const onTerminalOutputAvailable = vi.fn()
    await pull(
      { path: '/repo' } as Repository,
      { name: 'origin', url: '/remote' },
      {
        onHookProgress,
        onHookFailure,
        onTerminalOutputAvailable,
        ...(withProgress ? { progressCallback: vi.fn() } : {}),
      }
    )
    expect(mocks.git).toHaveBeenCalledOnce()
    expect(mocks.git.mock.calls[0][3]).toEqual(expect.objectContaining({
      onHookProgress,
      onHookFailure,
      onTerminalOutputAvailable,
      interceptHooks: expect.arrayContaining(['pre-merge-commit', 'pre-rebase', 'commit-msg']),
    }))
    expect(mocks.progress).toHaveBeenCalledTimes(withProgress ? 1 : 0)
  })
})
