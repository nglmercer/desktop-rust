import { afterEach, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({ pull: vi.fn() }))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    pullRepositoryRemote: mocks.pull,
    canonicalMutationPath: async (path: string) => path,
  }),
  FetchPreviewUnsupported: class extends Error {},
}))
vi.mock('../../src/lib/git/core', () => ({
  git: vi.fn(),
  gitRebaseArguments: () => [],
  getDescriptionForError: () => '',
  GitError: class extends Error {},
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForProxy: async () => undefined,
  envForRemoteOperation: async () => ({}),
}))
vi.mock('../../src/lib/git/config', () => ({
  getConfigValue: async () => null,
}))
vi.mock('../../src/lib/hooks/config', () => ({
  getHooksEnvEnabled: () => false,
}))
vi.mock('../../src/lib/git/fetch-error', () => ({
  fetchFailureToGitError: () => new Error('fetch failed'),
}))

import { pull } from '../../src/lib/git/pull'
import { hasRepositoryGitOperationAccess } from '../../src/lib/git/repository-operation'

afterEach(() => vi.clearAllMocks())

it('does not grant progress observers the pull operation ownership', async () => {
  const repository = {
    path: '/pull-ownership-test',
    gitHubRepository: null,
  } as Repository
  const observed: boolean[] = []
  mocks.pull.mockImplementationOnce(async (_path, _request, onProgress) => {
    expect(hasRepositoryGitOperationAccess(repository.path)).toBe(true)
    onProgress({
      stream: 'stderr',
      data: btoa('Receiving objects: 50% (1/2)\r'),
    })
  })
  await pull(
    repository,
    { name: 'origin', url: '/source' },
    {
      progressCallback: () =>
        observed.push(hasRepositoryGitOperationAccess(repository.path)),
    }
  )
  expect(observed.length).toBeGreaterThanOrEqual(2)
  expect(observed.every(value => value === false)).toBe(true)
  expect(hasRepositoryGitOperationAccess(repository.path)).toBe(false)
})
