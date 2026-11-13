import { afterEach, describe, expect, it, vi } from 'vitest'

const listeners = vi.hoisted(
  () => new Map<string, Set<(...args: unknown[]) => void>>()
)
const request = vi.hoisted(() => vi.fn(async () => ({ code: 0 })))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    capabilities: () => ({ cancel: true, max_pending: 32 }),
    waitReady: async () => ({ ready: true }),
    request,
    subscribe: (event: string, listener: (...args: unknown[]) => void) => {
      const set = listeners.get(event) ?? new Set()
      set.add(listener)
      listeners.set(event, set)
      return () => set.delete(listener)
    },
  }),
}))

interface IShim {
  invoke(channel: string, ...args: unknown[]): Promise<unknown>
  on(event: string, listener: (...args: unknown[]) => void): IShim
  removeListener(event: string, listener: (...args: unknown[]) => void): IShim
  removeAllListeners(event?: string): IShim
}

afterEach(() => {
  Reflect.deleteProperty(window, '__desktop')
  listeners.clear()
  request.mockClear()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.resetModules()
})

describe('loopback bridge event shim', () => {
  it('answers browser window queries locally and keeps typed operations on the bridge', async () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(true)
    vi.stubGlobal(
      'matchMedia',
      vi.fn(() => ({ matches: true }))
    )
    await import('../../src/web/electron-shim-loader')
    const shim = Reflect.get(window, '__desktop') as IShim
    await expect(
      shim.invoke('get-current-window-state')
    ).resolves.toBeUndefined()
    await expect(
      shim.invoke('get-current-window-zoom-factor')
    ).resolves.toBeUndefined()
    await expect(shim.invoke('get-app-path')).resolves.toBe('/__desktop_app__')
    await expect(shim.invoke('is-window-focused')).resolves.toBe(true)
    await expect(shim.invoke('should-use-dark-colors')).resolves.toBe(true)
    expect(window.matchMedia).toHaveBeenCalledWith(
      '(prefers-color-scheme: dark)'
    )
    expect(request).not.toHaveBeenCalled()
    await expect(
      shim.invoke('desktop.backend.path_exists', { path: '/tmp' })
    ).resolves.toEqual({ code: 0 })
    expect(request).toHaveBeenCalledWith('path_exists', { path: '/tmp' })
  })
  it('delivers subscriptions and removes exactly the requested listeners', async () => {
    await import('../../src/web/electron-shim-loader')
    const shim = Reflect.get(window, '__desktop') as IShim
    const first = vi.fn()
    const second = vi.fn()
    shim
      .on('repository_changed', first)
      .on('repository_changed', first)
      .on('repository_changed', second)
    for (const listener of listeners.get('repository_changed') ?? []) {
      listener({ root_id: 'one' })
    }
    expect(first).toHaveBeenCalledOnce()
    expect(first).toHaveBeenCalledWith({ sender: null }, { root_id: 'one' })
    expect(second).toHaveBeenCalledOnce()
    shim.removeListener('repository_changed', first)
    for (const listener of listeners.get('repository_changed') ?? []) {
      listener({ root_id: 'two' })
    }
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledTimes(2)
    shim.removeAllListeners()
    expect(listeners.get('repository_changed')?.size).toBe(0)
  })
  it('writes copy actions through the browser clipboard instead of the bridge', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })
    await import('../../src/web/electron-shim-loader')
    const shim = Reflect.get(window, '__desktop') as IShim
    await expect(
      shim.invoke('write-clipboard-text', 'hello')
    ).resolves.toBeUndefined()
    expect(writeText).toHaveBeenCalledWith('hello')
    expect(request).not.toHaveBeenCalled()
    writeText.mockRejectedValueOnce(new Error('permission denied'))
    await expect(shim.invoke('write-clipboard-text', 'hello')).rejects.toThrow(
      'permission denied'
    )
    await expect(shim.invoke('write-clipboard-text', 1)).rejects.toThrow(
      'Clipboard text must be a string'
    )
    Reflect.deleteProperty(navigator, 'clipboard')
    await expect(shim.invoke('write-clipboard-text', 'hello')).rejects.toThrow(
      'Clipboard access is unavailable'
    )
  })
  it('opens safe external links locally and rejects unsupported URLs', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    await import('../../src/web/electron-shim-loader')
    const shim = Reflect.get(window, '__desktop') as IShim
    await expect(
      shim.invoke('open-external', 'https://github.com/')
    ).resolves.toBe(true)
    expect(open).toHaveBeenCalledWith(
      'https://github.com/',
      '_blank',
      'noopener,noreferrer'
    )
    await expect(
      shim.invoke('open-external', 'mailto:test@example.com')
    ).resolves.toBe(true)
    await expect(
      shim.invoke('open-external', 'javascript:alert(1)')
    ).resolves.toBe(false)
    await expect(shim.invoke('open-external', '/relative')).resolves.toBe(false)
    await expect(shim.invoke('open-external', 1)).resolves.toBe(false)
    expect(open).toHaveBeenCalledTimes(2)
    expect(request).not.toHaveBeenCalled()
    open.mockImplementationOnce(() => {
      throw new Error('blocked')
    })
    await expect(
      shim.invoke('open-external', 'https://github.com/')
    ).resolves.toBe(false)
  })
})
