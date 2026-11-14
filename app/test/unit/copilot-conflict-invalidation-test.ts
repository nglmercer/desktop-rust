import { describe, expect, it, vi } from 'vitest'
import { AppStore } from '../../src/lib/stores/app-store'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../src/lib/custom-integration', () => ({
  TargetPathArgument: '$PATH',
  isValidCustomIntegration: vi.fn(),
}))
vi.mock('../../src/lib/git/repository-operation', async importOriginal => ({
  ...(await importOriginal<
    typeof import('../../src/lib/git/repository-operation')
  >()),
  withRepositoryGitOperation: async (
    _path: string,
    _kind: string,
    work: () => Promise<unknown>
  ) => work(),
}))

describe('Copilot suggestion invalidation', () => {
  for (const fail of [false, true]) {
    it(`invalidates used suggestions after ${
      fail ? 'failure' : 'success'
    }`, async () => {
      const repository = { path: '/repo' }
      const original = [
        { path: 'file', resolvedContent: 'resolved', reasoning: '' },
      ]
      let state = { copilotResolutions: original as typeof original | null }
      const cache = {
        get: () => ({ multiCommitOperationState: state }),
        updateMultiCommitOperationState: vi.fn((_repository, update) => {
          state = { ...state, ...update(state) }
        }),
      }
      const store = Object.create(AppStore.prototype)
      store.repositoryStateCache = cache
      store.assistedCommits = { state: { assertAvailable: vi.fn() } }
      store.withAssistedCommitMutationResources = async (
        _repository: unknown,
        work: () => Promise<unknown>
      ) => work()
      store.applyCopilotConflictResolutionsCore = fail
        ? vi.fn().mockRejectedValue(new Error('stale'))
        : vi.fn().mockResolvedValue(undefined)
      store.emitUpdate = vi.fn()
      if (fail)
        await expect(
          store._applyCopilotConflictResolutions(repository)
        ).rejects.toThrow('stale')
      else
        await expect(
          store._applyCopilotConflictResolutions(repository)
        ).resolves.toBeUndefined()
      expect(state.copilotResolutions).toBeNull()
      expect(cache.updateMultiCommitOperationState).toHaveBeenCalledOnce()
      expect(store.emitUpdate).toHaveBeenCalledOnce()
    })
  }

  it('does not invalidate newer suggestions published while an older operation ran', async () => {
    const repository = { path: '/repo' }
    const original = [{ path: 'first', resolvedContent: 'a', reasoning: '' }]
    const newer = [{ path: 'second', resolvedContent: 'b', reasoning: '' }]
    let state = { copilotResolutions: original }
    const cache = {
      get: () => ({ multiCommitOperationState: state }),
      updateMultiCommitOperationState: vi.fn(),
    }
    const store = Object.create(AppStore.prototype)
    store.repositoryStateCache = cache
    store.assistedCommits = { state: { assertAvailable: vi.fn() } }
    store.withAssistedCommitMutationResources = async (
      _repository: unknown,
      work: () => Promise<unknown>
    ) => work()
    store.applyCopilotConflictResolutionsCore = async () => {
      state = { copilotResolutions: newer }
    }
    store.emitUpdate = vi.fn()
    await store._applyCopilotConflictResolutions(repository)
    expect(state.copilotResolutions).toBe(newer)
    expect(cache.updateMultiCommitOperationState).not.toHaveBeenCalled()
  })
})
