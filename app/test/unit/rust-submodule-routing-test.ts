import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import type { WorkingDirectoryFileChange } from '../../src/models/status'
import { AppFileStatusKind } from '../../src/models/status'
import { DiffSelectionType, type ITextDiff } from '../../src/models/diff'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  status: vi.fn(),
  reset: vi.fn(),
  update: vi.fn(),
  checkoutIndex: vi.fn(),
  checkoutHead: vi.fn(),
  resetIndex: vi.fn(),
  mixedReset: vi.fn(),
  fastForward: vi.fn(),
  updateIndex: vi.fn(),
  applyPatch: vi.fn(),
  unstageAll: vi.fn(),
  removeIndexAll: vi.fn(),
  deleteInitialHead: vi.fn(),
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
    repositoryCheckoutIndex: mocks.checkoutIndex,
    repositoryCheckoutHeadPaths: mocks.checkoutHead,
    repositoryResetIndex: mocks.resetIndex,
    mixedResetRepository: mocks.mixedReset,
    fastForwardRepositoryBranches: mocks.fastForward,
    repositoryUpdateIndex: mocks.updateIndex,
    repositoryUnstageAll: mocks.unstageAll,
    repositoryRemoveIndexAll: mocks.removeIndexAll,
    repositoryDeleteInitialHead: mocks.deleteInitialHead,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/apply', () => ({
  applyPatchToIndex: mocks.applyPatch,
}))
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
import { checkoutIndex } from '../../src/lib/git/checkout-index'
import { GitResetMode, reset, resetPaths, unstageAll } from '../../src/lib/git/reset'
import { unstageAllFiles } from '../../src/lib/git/rm'
import { checkoutPaths } from '../../src/lib/git/checkout'
import { stageFiles } from '../../src/lib/git/update-index'
import { deleteRef } from '../../src/lib/git/update-ref'
import { fastForwardBranches } from '../../src/lib/git/fetch'

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
  it('restores selected index paths through the mutation scope', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.checkoutIndex
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('checkout failed'))
    await checkoutIndex(repository, [])
    expect(mocks.checkoutIndex).not.toHaveBeenCalled()
    await checkoutIndex(repository, ['one space', 'missing'])
    expect(mocks.checkoutIndex).toHaveBeenCalledWith('/repo', [
      'one space',
      'missing',
    ])
    await expect(checkoutIndex(repository, ['one space'])).rejects.toThrow(
      'checkout failed'
    )
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('checks out selected HEAD paths and preserves the empty selection', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.checkoutHead
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('checkout failed'))
    await checkoutPaths(repository, [])
    await checkoutPaths(repository, ['a\nb'])
    await expect(checkoutPaths(repository, ['missing'])).rejects.toThrow(
      'checkout failed'
    )
    expect(mocks.checkoutHead).toHaveBeenNthCalledWith(1, '/repo', [])
    expect(mocks.checkoutHead).toHaveBeenNthCalledWith(2, '/repo', ['a\nb'])
    expect(mocks.scope).toHaveBeenCalledTimes(3)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('batches large index selections before sending them', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.checkoutIndex.mockResolvedValue(undefined)
    const paths = Array.from({ length: 4100 }, (_, index) => `file-${index}`)
    await checkoutIndex(repository, paths)
    expect(mocks.checkoutIndex).toHaveBeenCalledTimes(2)
    expect(mocks.checkoutIndex.mock.calls.flatMap(call => call[1])).toEqual(
      paths
    )
    await expect(
      checkoutIndex(repository, ['valid', 'x'.repeat(50_000)])
    ).rejects.toThrow('request limit')
    expect(mocks.checkoutIndex).toHaveBeenCalledTimes(2)
  })
  it('resets selected paths to HEAD through Rust before restoring files', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.resetIndex
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('reset failed'))
    await resetPaths(repository, GitResetMode.Mixed, 'HEAD', [])
    expect(mocks.resetIndex).not.toHaveBeenCalled()
    await resetPaths(repository, GitResetMode.Mixed, 'HEAD', ['a\nb'])
    expect(mocks.resetIndex).toHaveBeenCalledWith('/repo', ['a\nb'])
    await expect(
      resetPaths(repository, GitResetMode.Mixed, 'HEAD', ['a'])
    ).rejects.toThrow('reset failed')
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('stages full selections in rename, update and deletion order', async () => {
    const repository = { path: '/repo' } as Repository
    const selected = { getSelectionType: () => DiffSelectionType.All }
    const partial = { getSelectionType: () => DiffSelectionType.Partial }
    const files = [
      {
        path: 'new',
        status: { kind: AppFileStatusKind.Renamed, oldPath: 'old' },
        selection: selected,
      },
      {
        path: 'gone',
        status: { kind: AppFileStatusKind.Deleted },
        selection: selected,
      },
      {
        path: 'partial',
        status: { kind: AppFileStatusKind.Modified },
        selection: partial,
      },
    ] as ReadonlyArray<WorkingDirectoryFileChange>
    mocks.updateIndex.mockResolvedValue(undefined)
    mocks.applyPatch.mockResolvedValue(undefined)
    await stageFiles(repository, files)
    expect(mocks.updateIndex.mock.calls).toEqual([
      ['/repo', ['old'], true],
      ['/repo', ['new', 'gone'], false],
      ['/repo', ['gone'], true],
    ])
    expect(mocks.applyPatch).toHaveBeenCalledWith(
      repository,
      files[2],
      undefined,
      false
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('retains a recreated rename source only when separately selected', async () => {
    const repository = { path: '/repo' } as Repository
    const partial = { getSelectionType: () => DiffSelectionType.Partial }
    const all = { getSelectionType: () => DiffSelectionType.All }
    const files = [
      {
        id: 'rename',
        path: 'new',
        status: { kind: AppFileStatusKind.Renamed, oldPath: 'old' },
        selection: partial,
      },
      {
        id: 'source',
        path: 'old',
        status: { kind: AppFileStatusKind.Untracked },
        selection: all,
      },
    ] as ReadonlyArray<WorkingDirectoryFileChange>
    const certified = new Map([['rename', {} as ITextDiff]])
    mocks.updateIndex.mockResolvedValue(undefined)
    mocks.applyPatch.mockResolvedValue(undefined)
    await stageFiles(repository, files, certified)
    expect(mocks.updateIndex).toHaveBeenCalledWith('/repo', ['old'], false)
    expect(mocks.applyPatch).toHaveBeenCalledWith(
      repository,
      files[0],
      certified.get('rename'),
      true
    )
  })
  it('stages a partial recreated source after its rename', async () => {
    const repository = { path: '/repo' } as Repository
    const partial = { getSelectionType: () => DiffSelectionType.Partial }
    const source = {
      id: 'source',
      path: 'old',
      status: { kind: AppFileStatusKind.Untracked },
      selection: partial,
    } as WorkingDirectoryFileChange
    const rename = {
      id: 'rename',
      path: 'new',
      status: { kind: AppFileStatusKind.Renamed, oldPath: 'old' },
      selection: partial,
    } as WorkingDirectoryFileChange
    const diff = {} as ITextDiff
    mocks.applyPatch.mockResolvedValue(undefined)
    await stageFiles(repository, [source, rename], new Map([['rename', diff]]))
    expect(mocks.applyPatch.mock.calls).toEqual([
      [repository, rename, diff, false],
      [repository, source, undefined, false],
    ])
    expect(mocks.updateIndex).not.toHaveBeenCalled()
  })
  it('rejects cyclic rename selections before changing the index', async () => {
    const repository = { path: '/repo' } as Repository
    const partial = { getSelectionType: () => DiffSelectionType.Partial }
    const files = [
      {
        id: 'a',
        path: 'a',
        status: { kind: AppFileStatusKind.Renamed, oldPath: 'b' },
        selection: partial,
      },
      {
        id: 'b',
        path: 'b',
        status: { kind: AppFileStatusKind.Renamed, oldPath: 'a' },
        selection: partial,
      },
    ] as ReadonlyArray<WorkingDirectoryFileChange>
    await expect(stageFiles(repository, files)).rejects.toThrow(
      'Overlapping renamed selections'
    )
    expect(mocks.updateIndex).not.toHaveBeenCalled()
    expect(mocks.applyPatch).not.toHaveBeenCalled()
  })
  it('unstages the index through Rust and propagates failures', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.unstageAll
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('unstage failed'))
    await expect(unstageAll(repository)).resolves.toBe(true)
    await expect(unstageAll(repository)).rejects.toThrow('unstage failed')
    expect(mocks.unstageAll).toHaveBeenCalledWith('/repo')
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('fast-forwards reviewed tracking branches through Rust', async () => {
    const repository = { path: '/repo' } as Repository
    const branches = [{ ref: 'refs/heads/feature', sha: 'a'.repeat(40), upstreamRef: 'refs/remotes/origin/feature', upstreamSha: 'b'.repeat(40) }]
    await fastForwardBranches(repository, [])
    expect(mocks.fastForward).not.toHaveBeenCalled()
    await fastForwardBranches(repository, branches, false)
    expect(mocks.fastForward).toHaveBeenCalledWith('/repo', branches, false)
    await fastForwardBranches(repository, branches, true)
    expect(mocks.fastForward).toHaveBeenLastCalledWith('/repo', branches, true)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('routes reviewed mixed reset and undo through Rust', async () => {
    const repository = { path: '/repo' } as Repository
    const target = 'a'.repeat(40)
    const expected = { head: 'b'.repeat(40), branch: 'refs/heads/main', undo: true }
    await expect(reset(repository, GitResetMode.Mixed, target, expected)).resolves.toBe(true)
    expect(mocks.mixedReset).toHaveBeenCalledWith('/repo', { target, ...expected })
    expect(mocks.git).not.toHaveBeenCalled()
    await expect(reset(repository, GitResetMode.Mixed, target)).rejects.toThrow('Refresh the current branch')
  })
  it('removes index entries without resetting to HEAD', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.removeIndexAll
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('remove failed'))
    await unstageAllFiles(repository)
    await expect(unstageAllFiles(repository)).rejects.toThrow('remove failed')
    expect(mocks.removeIndexAll).toHaveBeenCalledWith('/repo')
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('guards first-commit HEAD deletion with the selected commit', async () => {
    const repository = { path: '/repo' } as Repository
    const oid = 'a'.repeat(40)
    mocks.deleteInitialHead
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('HEAD changed'))
    await deleteRef(repository, 'HEAD', 'Reverting first commit', oid)
    await expect(
      deleteRef(repository, 'HEAD', 'Reverting first commit', oid)
    ).rejects.toThrow('HEAD changed')
    expect(mocks.deleteInitialHead).toHaveBeenCalledWith('/repo', oid)
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
