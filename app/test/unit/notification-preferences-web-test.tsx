import * as React from 'react'
import * as ReactDOM from 'react-dom'
import { act } from 'react-dom/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const native = vi.hoisted(() => ({
  supported: false,
  canRequest: false,
  settingsUrl: null as string | null,
  getPermission: vi.fn(),
  requestPermission: vi.fn(),
}))

vi.mock('desktop-notifications', () => ({
  supportsNotifications: () => native.supported,
  supportsNotificationsPermissionRequest: () => native.canRequest,
  getNotificationSettingsUrl: () => native.settingsUrl,
}))
vi.mock('../../src/ui/main-process-proxy', () => ({
  getNotificationsPermission: native.getPermission,
  requestNotificationsPermission: native.requestPermission,
}))
vi.mock('../../src/lib/app-shell', () => ({
  shell: { openExternal: vi.fn() },
}))

import { Notifications } from '../../src/ui/preferences/notifications'

describe('notification preferences', () => {
  let root: HTMLDivElement

  beforeEach(() => {
    native.supported = false
    native.canRequest = false
    native.settingsUrl = null
    native.getPermission.mockReset()
    native.requestPermission.mockReset()
    root = document.createElement('div')
    document.body.appendChild(root)
  })

  afterEach(() => {
    ReactDOM.unmountComponentAtNode(root)
    root.remove()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  async function render(enabled = true) {
    await act(async () => {
      ReactDOM.render(
        <Notifications
          notificationsEnabled={enabled}
          onNotificationsEnabledChanged={() => {}}
        />,
        root
      )
    })
  }

  it('requests browser permission only after a click and coalesces repeated clicks', async () => {
    let permission: NotificationPermission = 'default'
    let resolveRequest: ((value: NotificationPermission) => void) | undefined
    const request = vi.fn(
      () =>
        new Promise<NotificationPermission>(resolve => {
          resolveRequest = resolve
        })
    )
    vi.stubGlobal('Notification', {
      get permission() {
        return permission
      },
      requestPermission: request,
    })
    await render()
    expect(native.getPermission).not.toHaveBeenCalled()
    expect(request).not.toHaveBeenCalled()
    const link = root.querySelector('a[role="button"]')
    expect(link?.textContent).toBe('grant permission')
    await act(async () => {
      link?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      link?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    expect(request).toHaveBeenCalledTimes(1)
    permission = 'denied'
    await act(async () => {
      resolveRequest?.('denied')
      await Promise.resolve()
    })
    expect(root.textContent).toContain('Notifications are blocked')
    expect(root.querySelector('a[role="button"]')).toBeNull()
    expect(native.requestPermission).not.toHaveBeenCalled()
  })

  it('refreshes browser permission after the window regains focus', async () => {
    let permission: NotificationPermission = 'denied'
    vi.stubGlobal('Notification', {
      get permission() {
        return permission
      },
      requestPermission: vi.fn(),
    })
    await render()
    expect(root.textContent).toContain('Notifications are blocked')
    permission = 'granted'
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
      await Promise.resolve()
    })
    expect(root.textContent).not.toContain('Notifications are blocked')
  })

  it('does not offer permission when the browser API is unavailable or notifications are disabled', async () => {
    vi.stubGlobal('Notification', undefined)
    await render()
    expect(root.textContent).not.toContain('grant permission')
    expect(native.getPermission).not.toHaveBeenCalled()
    vi.stubGlobal('Notification', {
      permission: 'default',
      requestPermission: vi.fn(),
    })
    await render(false)
    expect(root.textContent).not.toContain('grant permission')
  })

  it('keeps the native permission flow for supported Electron platforms', async () => {
    native.supported = true
    native.canRequest = true
    native.settingsUrl = 'ms-settings:notifications'
    native.getPermission.mockResolvedValue('default')
    native.requestPermission.mockResolvedValue(true)
    const request = vi.fn()
    vi.stubGlobal('Notification', {
      permission: 'default',
      requestPermission: request,
    })
    await render()
    const link = root.querySelector('a[role="button"]')
    expect(link?.textContent).toBe('grant permission')
    native.getPermission.mockResolvedValue('granted')
    await act(async () => {
      link?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })
    expect(native.requestPermission).toHaveBeenCalledTimes(1)
    expect(request).not.toHaveBeenCalled()
    expect(root.textContent).toContain('properly configured')
  })

  it('allows retrying after a browser permission request fails', async () => {
    const request = vi.fn().mockRejectedValue(new Error('permission failed'))
    vi.stubGlobal('Notification', {
      permission: 'default',
      requestPermission: request,
    })
    const warn = vi.spyOn(log, 'warn')
    await render()
    const link = root.querySelector('a[role="button"]')
    await act(async () => {
      link?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })
    expect(warn).toHaveBeenCalledOnce()
    expect(root.querySelector('a[role="button"]')?.textContent).toBe(
      'grant permission'
    )
    await act(async () => {
      root
        .querySelector('a[role="button"]')
        ?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })
    expect(request).toHaveBeenCalledTimes(2)
  })

  it('does not let an older native permission lookup overwrite a newer one', async () => {
    native.supported = true
    native.settingsUrl = 'ms-settings:notifications'
    let resolveOld: ((value: NotificationPermission) => void) | undefined
    native.getPermission
      .mockImplementationOnce(
        () =>
          new Promise(resolve => {
            resolveOld = resolve
          })
      )
      .mockResolvedValueOnce('denied')
    await render()
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
      await Promise.resolve()
    })
    expect(root.textContent).toContain('has no permission')
    await act(async () => {
      resolveOld?.('granted')
      await Promise.resolve()
    })
    expect(root.textContent).toContain('has no permission')
    expect(root.textContent).not.toContain('properly configured')
  })
})
