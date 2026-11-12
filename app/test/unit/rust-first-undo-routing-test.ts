import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import type { Commit } from '../../src/models/commit'
import type { IAppShell } from '../../src/lib/app-shell'
import type { IStatsStore } from '../../src/lib/stats'
import { AppFileStatusKind } from '../../src/models/status'

const mocks = vi.hoisted(() => ({
  status: vi.fn(),
  checkout: vi.fn(),
  deleteRef: vi.fn(),
  clearIndex: vi.fn(),
}))
vi.mock('keytar', () => ({
  getPassword: vi.fn(),
  setPassword: vi.fn(),
  deletePassword: vi.fn(),
}))
vi.mock('../../src/lib/trampoline/trampoline-environment', () => ({
  withTrampolineEnv: vi.fn(),
  getIsBackgroundTaskEnvironment: vi.fn(),
  getDesktopAskpassTrampolinePath: vi.fn(),
  getSSHWrapperPath: vi.fn(),
  getTrampolineEnvironmentPath: vi.fn(),
  getTrampolineFallbackAccount: vi.fn(),
  holdCredentialLease: vi.fn(),
  setHasRejectedCredentialsForEndpoint: vi.fn(),
}))
vi.mock('../../src/lib/git', () => ({
  getStatus: mocks.status,
  checkoutPaths: mocks.checkout,
  deleteRef: mocks.deleteRef,
  unstageAllFiles: mocks.clearIndex,
  GitError: {},
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: async (
    _path: string,
    _kind: string,
    run: () => Promise<unknown>
  ) => run(),
  withoutRepositoryGitAccess: (run: () => unknown) => run(),
  getRepositoryGitErrorPropagation: () => false,
}))
import { GitStore } from '../../src/lib/stores/git-store'

describe('first-commit undo routing', () => {
  const repository = {
    id: 42,
    path: '/repo',
    gitHubRepository: null,
  } as Repository
  const oid = 'a'.repeat(40)
  const commit = {
    sha: oid,
    parentSHAs: [],
    summary: 'first',
    body: '',
  } as Commit
  beforeEach(() => vi.clearAllMocks())

  it.each([
    ['a different tip', 'b'.repeat(40)],
    ['an unborn branch', undefined],
  ])('leaves files and refs untouched with %s', async (_description, tip) => {
    mocks.status.mockResolvedValue({
      currentTip: tip,
      workingDirectory: {
        files: [
          { path: 'deleted', status: { kind: AppFileStatusKind.Deleted } },
        ],
      },
    })
    const store = new GitStore(repository, {} as IAppShell, {} as IStatsStore)
    const error = vi.fn()
    store.onDidError(error)
    await store.undoCommit(commit)
    expect(error).toHaveBeenCalledOnce()
    expect(mocks.checkout).not.toHaveBeenCalled()
    expect(mocks.deleteRef).not.toHaveBeenCalled()
    expect(mocks.clearIndex).not.toHaveBeenCalled()
  })

  it('restores deleted paths before deleting the selected tip and clearing the index', async () => {
    mocks.status.mockResolvedValue({
      currentTip: oid,
      workingDirectory: {
        files: [
          { path: 'deleted', status: { kind: AppFileStatusKind.Deleted } },
        ],
      },
    })
    const store = new GitStore(repository, {} as IAppShell, {} as IStatsStore)
    await store.undoCommit(commit)
    expect(mocks.checkout).toHaveBeenCalledWith(repository, ['deleted'])
    expect(mocks.deleteRef).toHaveBeenCalledWith(
      repository,
      'HEAD',
      'Reverting first commit',
      oid
    )
    expect(mocks.clearIndex).toHaveBeenCalledWith(repository)
    expect(mocks.checkout.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.deleteRef.mock.invocationCallOrder[0]
    )
    expect(mocks.deleteRef.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.clearIndex.mock.invocationCallOrder[0]
    )
  })
})
