import { describe, expect, it, vi } from 'vitest'
import { RepositorySelectionQueue } from '../../src/backend/repository-selection'

describe('browser repository selection', () => {
  it('serializes choices and prevents an older completion from winning', async () => {
    let finish!: () => void
    const apply = vi.fn(async (path: string | null) => {
      if (path === '/first')
        await new Promise<void>(resolve => {
          finish = resolve
        })
    })
    const queue = new RepositorySelectionQueue(apply)
    const first = queue.select('/first')
    await vi.waitFor(() => expect(apply).toHaveBeenCalledWith('/first'))
    const second = queue.select('/second')
    expect(apply).toHaveBeenCalledTimes(1)
    finish()
    await expect(first).resolves.toBe(false)
    await expect(second).resolves.toBe(true)
    expect(apply.mock.calls).toEqual([['/first'], ['/second']])
  })

  it('clears uncertain bindings after a selection failure and permits a retry', async () => {
    const failure = new Error('selection failed')
    const apply = vi.fn(async (path: string | null) => {
      if (path === '/bad') throw failure
    })
    const queue = new RepositorySelectionQueue(apply)
    await expect(queue.select('/bad')).rejects.toBe(failure)
    await expect(queue.select('/good')).resolves.toBe(true)
    expect(apply.mock.calls).toEqual([['/bad'], [null], ['/good']])
  })

  it('skips choices superseded before they start', async () => {
    const apply = vi.fn(async (_path: string | null) => {})
    const queue = new RepositorySelectionQueue(apply)
    const first = queue.select('/first')
    const second = queue.select(null)
    await expect(first).resolves.toBe(false)
    await expect(second).resolves.toBe(true)
    expect(apply.mock.calls).toEqual([[null]])
  })
})
