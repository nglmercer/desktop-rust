import { describe, expect, it, vi } from 'vitest'
import { AppStore } from '../../../src/lib/stores/app-store'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({
  TargetPathArgument: '$PATH',
  isValidCustomIntegration: vi.fn(),
}))

describe('remote branch deletion cleanup', () => {
  it('refreshes after a partial failure without masking the original error', async () => {
    const failure = new Error('second push destination failed')
    const repository = { path: '/repo' }
    const review = { local: { name: 'topic' } }
    const events: string[] = []
    const store = Object.create(AppStore.prototype)
    store.withAssistedCommitMutationResources = async () => {
      events.push('delete')
      throw failure
    }
    store._refreshRepository = async (actual: unknown) => {
      expect(actual).toBe(repository)
      events.push('refresh')
      throw new Error('refresh failed')
    }

    await expect(store._deleteReviewedRemoteBranch(repository, review)).rejects.toBe(failure)
    expect(events).toEqual(['delete', 'refresh'])
  })
})
