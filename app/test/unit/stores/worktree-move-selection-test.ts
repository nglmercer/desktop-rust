import { describe, expect, it, vi } from 'vitest'
import { Repository, getRepositoryStateKey } from '../../../src/models/repository'
import { AppStore } from '../../../src/lib/stores/app-store'

const mocks = vi.hoisted(() => ({ move: vi.fn(), repositoryType: vi.fn() }))
vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({ TargetPathArgument: '$PATH', isValidCustomIntegration: vi.fn() }))
vi.mock('../../../src/lib/git', async importOriginal => ({
  ...(await importOriginal<typeof import('../../../src/lib/git')>()),
  moveWorktree: mocks.move,
  getRepositoryType: mocks.repositoryType,
}))

describe('moving the selected worktree', () => {
  it('does not auto-select a moved path while a newer selection is pending', () => {
    const oldRepo = new Repository('/old', 1, null, false)
    const otherRepo = new Repository('/other', 2, null, false)
    const movedRepo = new Repository('/new', 1, null, false)
    const store = Object.create(AppStore.prototype)
    store.selectedRepository = oldRepo
    store.requestedRepositorySelection = otherRepo
    store.repositories = [movedRepo, otherRepo]
    store.movingWorktrees = new Set()
    store._selectRepository = vi.fn()
    store.updateRepositorySelectionAfterRepositoriesChanged()
    expect(store._selectRepository).not.toHaveBeenCalled()
  })
  it('does not override a newer selection while switching worktrees', async () => {
    const oldRepo = new Repository('/old', 1, null, false)
    const mainRepo = new Repository('/main', 1, null, false)
    let finish!: (value: unknown) => void
    mocks.repositoryType.mockReturnValueOnce(new Promise(resolve => { finish = resolve }))
    const store = Object.create(AppStore.prototype)
    store.repositorySelectionRevision = 1
    store.movingWorktrees = new Set()
    store.findMainWorktreePath = vi.fn().mockResolvedValue('/main')
    store.repositoriesStore = { switchWorktree: vi.fn().mockResolvedValue({ repository: mainRepo }) }
    store.repositoryStateCache = { seedFromWorktree: vi.fn() }
    store.adoptRepository = vi.fn()
    store._selectRepository = vi.fn()
    store.statsStore = { increment: vi.fn() }
    const pending = store.switchWorktreeCore(oldRepo, { path: '/main' })
    store.repositorySelectionRevision++
    finish({ kind: 'regular', gitDir: '/main/.git' })
    await pending
    expect(store._selectRepository).not.toHaveBeenCalled()
  })
  it('relocates the saved record of a separately saved worktree', async () => {
    const main = new Repository('/main', 1, null, false)
    const oldRepo = new Repository('/old', 2, null, false)
    const movedRepo = new Repository('/new', 2, null, false)
    mocks.move.mockResolvedValueOnce(undefined)
    const store = Object.create(AppStore.prototype)
    store.repositories = [main, oldRepo]
    store.repositoriesStore = { switchWorktree: vi.fn().mockResolvedValue({ repository: movedRepo, existingRepository: false }) }
    store.repositoryStateCache = { transferState: vi.fn() }
    store.gitStoreCache = { transfer: vi.fn() }
    store.adoptRepository = vi.fn()
    store._refreshWorktrees = vi.fn().mockResolvedValue(undefined)
    await store.moveWorktreeCore(main, '/old', '/new')
    expect(store.repositoriesStore.switchWorktree).toHaveBeenCalledWith(oldRepo, '/new')
    expect(store.repositoryStateCache.transferState).toHaveBeenCalledWith(oldRepo, movedRepo)
  })

  it('rejects a separately saved destination before moving', async () => {
    const main = new Repository('/main', 1, null, false)
    const store = Object.create(AppStore.prototype)
    store.repositories = [main, new Repository('/old', 2, null, false), new Repository('/new', 3, null, false)]
    mocks.move.mockClear()
    await expect(store.moveWorktreeCore(main, '/old', '/new')).rejects.toThrow('already saved')
    expect(mocks.move).not.toHaveBeenCalled()
  })

  it('reports a saved-record failure as a completed move needing inspection', async () => {
    const oldRepo = new Repository('/old', 1, null, false)
    mocks.move.mockResolvedValueOnce(undefined)
    const store = Object.create(AppStore.prototype)
    store.repositorySelectionRevision = 1
    store.selectedRepository = oldRepo
    store.repositoriesStore = { switchWorktree: vi.fn().mockRejectedValue(new Error('database unavailable')) }
    await expect(store.moveCurrentWorktree(oldRepo, '/old', '/new')).rejects.toThrow('worktree moved, but its saved repository could not be updated')
  })
  it('does not auto-select a moved repository while its saved path is being updated', () => {
    const oldRepo = new Repository('/old', 1, null, false)
    const movedRepo = new Repository('/new', 1, null, false)
    const store = Object.create(AppStore.prototype)
    store.selectedRepository = oldRepo
    store.repositories = [movedRepo]
    store.movingWorktrees = new Set([getRepositoryStateKey(oldRepo)])
    store._selectRepository = vi.fn()
    store.updateRepositorySelectionAfterRepositoriesChanged()
    expect(store._selectRepository).not.toHaveBeenCalled()
  })
  it.each([true, false])('follows the moved path only if selection stays current: %s', async changeSelection => {
    const oldRepo = new Repository('/old', 1, null, false)
    const movedRepo = new Repository('/new', 1, null, false)
    const otherRepo = new Repository('/other', 2, null, false)
    let finish!: () => void
    mocks.move.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    const store = Object.create(AppStore.prototype)
    store.repositorySelectionRevision = 1
    store.selectedRepository = oldRepo
    store.repositoriesStore = { switchWorktree: vi.fn().mockResolvedValue({ repository: movedRepo }) }
    store.repositoryStateCache = { transferState: vi.fn() }
    store.gitStoreCache = { transfer: vi.fn() }
    store.adoptRepository = vi.fn()
    store._selectRepository = vi.fn().mockResolvedValue(movedRepo)
    store._refreshWorktrees = vi.fn().mockResolvedValue(undefined)
    const pending = store.moveCurrentWorktree(oldRepo, '/old', '/new')
    if (changeSelection) {
      store.repositorySelectionRevision++
      store.selectedRepository = otherRepo
    }
    finish()
    await pending
    expect(store.repositoriesStore.switchWorktree).toHaveBeenCalledWith(oldRepo, '/new')
    expect(store._selectRepository).toHaveBeenCalledTimes(changeSelection ? 0 : 1)
  })
})
