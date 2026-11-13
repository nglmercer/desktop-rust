import assert from 'node:assert'
import { afterEach, describe, it, onTestFinished, vi } from 'vitest'
import { ipcRenderer } from 'electron'
import { MockIPC } from '../helpers/mock-ipc'

let nativeNotificationsSupported = true
vi.mock('desktop-notifications', () => ({
  supportsNotifications: () => nativeNotificationsSupported,
}))

async function getSystemNotificationsPermission() {
  const module = await import(
    '../../src/lib/notifications/notification-permission'
  )
  return module.getSystemNotificationsPermission()
}

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('system notification permission', () => {
  for (const permission of ['granted', 'denied', 'default'] as const) {
    for (const windows of [false, true]) {
      it(`normalizes native ${permission} permission on ${
        windows ? 'Windows' : 'macOS'
      } without requesting it`, async () => {
        const previousWindows = __WIN32__
        Object.assign(globalThis, { __WIN32__: windows })
        onTestFinished(() => {
          Object.assign(globalThis, { __WIN32__: previousWindows })
        })
        nativeNotificationsSupported = true
        const ipc = new MockIPC()
        ipc.onInvoke('get-notifications-permission', async () => permission)
        vi.spyOn(ipcRenderer, 'invoke').mockImplementation(
          ipc.invoke.bind(ipc) as typeof ipcRenderer.invoke
        )

        assert.strictEqual(
          await getSystemNotificationsPermission(),
          permission === 'granted' || (windows && permission === 'default')
        )
        assert.deepStrictEqual(
          ipc.invokes.map(call => call.channel),
          ['get-notifications-permission']
        )
      })
    }

    it(`reads HTML5 ${permission} permission without native IPC`, async () => {
      nativeNotificationsSupported = false
      vi.stubGlobal('Notification', { permission })
      const invoke = vi.spyOn(ipcRenderer, 'invoke')

      assert.strictEqual(
        await getSystemNotificationsPermission(),
        permission === 'granted'
      )
      assert.strictEqual(invoke.mock.calls.length, 0)
    })
  }

  it('reports null when neither notification API is available', async () => {
    nativeNotificationsSupported = false
    vi.stubGlobal('Notification', undefined)
    assert.strictEqual(await getSystemNotificationsPermission(), null)
  })

  it('reports null and logs native permission lookup failures', async () => {
    nativeNotificationsSupported = true
    const error = new Error('Permission lookup failed')
    vi.spyOn(ipcRenderer, 'invoke').mockImplementation(async () => {
      throw error
    })
    const warn = vi.spyOn(log, 'warn')

    assert.strictEqual(await getSystemNotificationsPermission(), null)
    assert.deepStrictEqual(warn.mock.calls[0], [
      'Failed to read system notification permission',
      error,
    ])
  })

  it('reports null and logs HTML5 permission lookup failures', async () => {
    nativeNotificationsSupported = false
    const error = new Error('Permission lookup failed')
    vi.stubGlobal('Notification', {
      get permission() {
        throw error
      },
    })
    const warn = vi.spyOn(log, 'warn')

    assert.strictEqual(await getSystemNotificationsPermission(), null)
    assert.deepStrictEqual(warn.mock.calls[0], [
      'Failed to read system notification permission',
      error,
    ])
  })
})
