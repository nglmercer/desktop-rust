import { describe, expect, it, vi } from 'vitest'
import { AppStore } from '../../../src/lib/stores/app-store'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({
  TargetPathArgument: '$PATH',
  isValidCustomIntegration: vi.fn(),
}))

describe('revert cleanup', () => {
  it('clears progress and refreshes after a conflict without masking it', async () => {
    const conflict = new Error('revert conflict')
    const refreshFailure = new Error('refresh failed')
    const events: string[] = []
    const repository = { path: '/repo' }
    const commit = { sha: 'a'.repeat(40) }
    const store = Object.create(AppStore.prototype)
    store.withRefreshedGitHubRepository = (
      _repo: unknown,
      fn: (repo: unknown) => unknown
    ) => fn(repository)
    store.gitStoreCache = {
      get: () => ({
        revertCommit: async (
          _repo: unknown,
          _commit: unknown,
          progress: (value: unknown) => void
        ) => {
          progress('started')
          throw conflict
        },
      }),
    }
    store.updateRevertProgress = (_repo: unknown, progress: unknown) =>
      events.push(progress === null ? 'clear' : 'progress')
    store._refreshRepository = async () => {
      events.push('refresh')
      throw refreshFailure
    }

    await expect(store.revertCommitCore(repository, commit)).rejects.toBe(conflict)
    expect(events).toEqual(['progress', 'clear', 'refresh'])
  })

  it('clears progress and refreshes on success and reports refresh failures', async () => {
    const events: string[] = []
    const refreshFailure = new Error('refresh failed')
    const repository = { path: '/repo' }
    const store = Object.create(AppStore.prototype)
    store.withRefreshedGitHubRepository = (
      _repo: unknown,
      fn: (repo: unknown) => unknown
    ) => fn(repository)
    store.gitStoreCache = {
      get: () => ({ revertCommit: async () => { events.push('revert') } }),
    }
    store.updateRevertProgress = () => events.push('clear')
    store._refreshRepository = async () => {
      events.push('refresh')
    }

    await expect(store.revertCommitCore(repository, {})).resolves.toBeUndefined()
    expect(events).toEqual(['revert', 'clear', 'refresh'])
    events.length = 0
    store._refreshRepository = async () => {
      events.push('refresh')
      throw refreshFailure
    }

    await expect(store.revertCommitCore(repository, {})).rejects.toBe(refreshFailure)
    expect(events).toEqual(['revert', 'clear', 'refresh'])
  })

  it('still refreshes if clearing progress fails', async () => {
    const failure = new Error('clear failed')
    const repository = { path: '/repo' }
    const refresh = vi.fn()
    const store = Object.create(AppStore.prototype)
    store.withRefreshedGitHubRepository = (
      _repo: unknown,
      fn: (repo: unknown) => unknown
    ) => fn(repository)
    store.gitStoreCache = { get: () => ({ revertCommit: async () => {} }) }
    store.updateRevertProgress = () => {
      throw failure
    }
    store._refreshRepository = refresh

    await expect(store.revertCommitCore(repository, {})).rejects.toBe(failure)
    expect(refresh).toHaveBeenCalledOnce()
  })
})
