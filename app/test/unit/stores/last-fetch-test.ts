import { describe, expect, it, vi } from 'vitest'
import { GitStore } from '../../../src/lib/stores/git-store'
import { BackendError } from '../../../src/backend/client'

const service = vi.hoisted(() => ({ read: vi.fn() }))
vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({ TargetPathArgument: '$PATH', isValidCustomIntegration: vi.fn() }))
vi.mock('../../../src/backend/client', async importOriginal => ({ ...await importOriginal<typeof import('../../../src/backend/client')>(), backend: () => ({ transportKind: 'ws', repositoryLastFetch: service.read }) }))

describe('last fetch display', () => {
  it('updates the displayed time, retains it for an empty fetch and clears it when absent', async () => {
    const store = Object.create(GitStore.prototype)
    store._repository = { path: '/repo' }
    store.emitUpdate = vi.fn()
    store._lastFetched = null
    service.read.mockResolvedValueOnce({ kind: 'fetched', time: 1_700_000_000_000 }).mockResolvedValueOnce({ kind: 'empty' }).mockRejectedValueOnce(new BackendError({ code: 'disconnected', message: 'disconnected', retryable: true })).mockRejectedValueOnce(new BackendError({ code: 'invalid_response', message: 'invalid response', retryable: false })).mockResolvedValueOnce({ kind: 'unavailable' })
    await expect(store.updateLastFetchedReadCore()).resolves.toEqual(new Date(1_700_000_000_000))
    await expect(store.updateLastFetchedReadCore()).resolves.toEqual(new Date(1_700_000_000_000))
    await expect(store.updateLastFetchedReadCore()).resolves.toEqual(new Date(1_700_000_000_000))
    await expect(store.updateLastFetchedReadCore()).resolves.toEqual(new Date(1_700_000_000_000))
    await expect(store.updateLastFetchedReadCore()).resolves.toBeNull()
    expect(service.read).toHaveBeenCalledWith('/repo')
    expect(store.emitUpdate).toHaveBeenCalledTimes(5)
  })
})
