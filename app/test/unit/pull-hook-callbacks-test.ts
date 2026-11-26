import { describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  pull: vi.fn(),
  withHooksEnv: vi.fn(),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    pullRepositoryRemote: mocks.pull,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({
  getDescriptionForError: () => undefined,
  GitError: class extends Error {},
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForProxy: async () => undefined,
  backendRemoteProxy: () => undefined,
}))
vi.mock('../../src/lib/hooks/with-hooks-env', () => ({
  withHooksEnv: mocks.withHooksEnv,
  rendererHookEnvironment: () => undefined,
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: async (
    _path: string,
    _kind: string,
    run: () => Promise<unknown>
  ) => run(),
  withoutRepositoryGitAccess: (run: () => void) => run(),
}))
vi.mock('../../src/lib/git/fetch-error', () => ({
  fetchFailureToGitError: () => new Error('fetch failed'),
}))

import { pull } from '../../src/lib/git/pull'

describe('pull hook callbacks', () => {
  it.each([false, true])(
    'passes callbacks to the typed hook environment with progress enabled: %s',
    async withProgress => {
      mocks.pull.mockReset().mockResolvedValue(undefined)
      let hookOptions: unknown
      mocks.withHooksEnv
        .mockReset()
        .mockImplementation(
          async (
            run: () => Promise<unknown>,
            _path: string,
            options: unknown
          ) => {
            hookOptions = options
            return run()
          }
        )
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
      expect(hookOptions).toEqual(
        expect.objectContaining({
          onHookProgress,
          onHookFailure,
          interceptHooks: expect.arrayContaining([
            'pre-merge-commit',
            'pre-rebase',
            'commit-msg',
          ]),
        })
      )
      expect(mocks.pull).toHaveBeenCalledWith(
        '/repo',
        expect.objectContaining({ report_progress: true }),
        expect.any(Function),
        expect.any(Function)
      )
    }
  )
})
