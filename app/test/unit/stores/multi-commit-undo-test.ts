import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AppStore } from '../../../src/lib/stores/app-store'

const mocks = vi.hoisted(() => ({ checkout: vi.fn() }))
vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({ TargetPathArgument: '$PATH', isValidCustomIntegration: vi.fn() }))
vi.mock('../../../src/lib/git/checkout', async importOriginal => ({ ...await importOriginal<typeof import('../../../src/lib/git/checkout')>(), checkoutBranch: mocks.checkout }))

describe('multi-commit undo', () => {
  const repository = { path: '/repo' }
  const expected = 'a'.repeat(40)
  const branch = { name: 'topic', ref: 'refs/heads/topic', tip: { sha: expected } }
  const operation = { operationDetail: { kind: 'Cherry-pick', branchCreated: true, sourceBranch: { name: 'main' } } }
  let store: any

  beforeEach(() => {
    mocks.checkout.mockReset()
    mocks.checkout.mockResolvedValue(true)
    store = Object.create(AppStore.prototype)
    store.repositoryStateCache = { get: () => ({ branchesState: { tip: { kind: 'Valid', branch } }, multiCommitOperationUndoState: { undoSha: 'b'.repeat(40), branchName: 'topic' }, changesState: { workingDirectory: { files: [] } } }) }
    store.gitStoreCache = { get: () => ({ currentRemote: null, performFailableOperation: async (fn: () => Promise<unknown>) => { try { return await fn() } catch { return undefined } } }) }
    store.deleteLocalBranchAndUpstreamBranch = vi.fn().mockResolvedValue(undefined)
    store._refreshRepository = vi.fn().mockResolvedValue(undefined)
  })

  it('waits for branch deletion before reporting success', async () => {
    let finish!: () => void
    store.deleteLocalBranchAndUpstreamBranch.mockImplementation(() => new Promise<void>(resolve => { finish = resolve }))
    const undo = store.undoMultiCommitOperationCore(operation, repository, 1)
    await vi.waitFor(() => expect(store.deleteLocalBranchAndUpstreamBranch).toHaveBeenCalled())
    let finished = false
    void undo.then(() => { finished = true })
    await Promise.resolve()
    expect(finished).toBe(false)
    finish()
    await expect(undo).resolves.toBe(true)
    expect(store._refreshRepository).toHaveBeenCalledWith(repository)
  })

  it('reports failure and refreshes if branch deletion fails', async () => {
    store.deleteLocalBranchAndUpstreamBranch.mockRejectedValue(new Error('delete failed'))
    await expect(store.undoMultiCommitOperationCore(operation, repository, 1)).resolves.toBe(false)
    expect(store._refreshRepository).toHaveBeenCalledWith(repository)
  })

  it('does not delete the branch when checkout fails', async () => {
    mocks.checkout.mockRejectedValue(new Error('checkout failed'))
    await expect(store.undoMultiCommitOperationCore(operation, repository, 1)).resolves.toBe(false)
    expect(store.deleteLocalBranchAndUpstreamBranch).not.toHaveBeenCalled()
    expect(store._refreshRepository).toHaveBeenCalledWith(repository)
  })
})
