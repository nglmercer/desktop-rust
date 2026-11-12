import { afterEach, describe, expect, it, vi } from 'vitest'

const listeners = vi.hoisted(
  () => new Map<string, Set<(...args: unknown[]) => void>>()
)
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    capabilities: () => ({ cancel: true, max_pending: 32 }),
    waitReady: async () => ({ ready: true }),
    subscribe: (event: string, listener: (...args: unknown[]) => void) => {
      const set = listeners.get(event) ?? new Set()
      set.add(listener)
      listeners.set(event, set)
      return () => set.delete(listener)
    },
  }),
}))

interface IShim {
  on(event: string, listener: (...args: unknown[]) => void): IShim
  removeListener(event: string, listener: (...args: unknown[]) => void): IShim
  removeAllListeners(event?: string): IShim
}

afterEach(() => {
  Reflect.deleteProperty(window, '__desktop')
  listeners.clear()
  vi.resetModules()
})

describe('loopback bridge event shim', () => {
  it('delivers subscriptions and removes exactly the requested listeners', async () => {
    await import('../../src/web/electron-shim-loader')
    const shim = Reflect.get(window, '__desktop') as IShim
    const first = vi.fn()
    const second = vi.fn()
    shim
      .on('repository_changed', first)
      .on('repository_changed', first)
      .on('repository_changed', second)
    for (const listener of listeners.get('repository_changed') ?? [])
      listener({ root_id: 'one' })
    expect(first).toHaveBeenCalledOnce()
    expect(first).toHaveBeenCalledWith({ sender: null }, { root_id: 'one' })
    expect(second).toHaveBeenCalledOnce()
    shim.removeListener('repository_changed', first)
    for (const listener of listeners.get('repository_changed') ?? [])
      listener({ root_id: 'two' })
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledTimes(2)
    shim.removeAllListeners()
    expect(listeners.get('repository_changed')?.size).toBe(0)
  })
})
