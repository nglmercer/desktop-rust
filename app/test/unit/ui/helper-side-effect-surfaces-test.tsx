import assert from 'node:assert'
import { afterEach, describe, it } from 'vitest'
import * as React from 'react'

import { AppTheme } from '../../../src/ui/app-theme'
import { ApplicationTheme } from '../../../src/ui/lib/application-theme'
import { render } from '../../helpers/ui/render'

afterEach(async () => {
  document.body.className = ''
  document.body.style.removeProperty('--background-color')
  document.documentElement.style.colorScheme = ''
})

describe('helper side-effect surfaces', () => {
  it('applies theme classes, updates color scheme, and clears theme classes on unmount', async () => {
    const hostBridge = await import('../../../src/web/host-bridge')
    const previousSend = hostBridge.hostIpc.send
    const sends: Array<[string, string]> = []

    hostBridge.hostIpc.send = (channel: string, value: string) => {
      sends.push([channel, value])
    }

    try {
      document.body.style.setProperty('--background-color', 'rgb(1, 2, 3)')

      const view = render(<AppTheme theme={ApplicationTheme.Dark} />)

      assert.ok(document.body.classList.contains('theme-dark'))
      assert.equal(document.documentElement.style.colorScheme, 'dark')
      assert.deepEqual(sends, [
        ['update-window-background-color', 'rgb(1, 2, 3)'],
      ])

      view.rerender(<AppTheme theme={ApplicationTheme.Light} />)

      assert.equal(document.body.classList.contains('theme-dark'), false)
      assert.ok(document.body.classList.contains('theme-light'))
      assert.equal(document.documentElement.style.colorScheme, 'light')
      assert.deepEqual(sends.at(-1), [
        'update-window-background-color',
        'rgb(1, 2, 3)',
      ])

      view.unmount()

      assert.equal(document.body.classList.contains('theme-light'), false)
    } finally {
      hostBridge.hostIpc.send = previousSend
    }
  })
})
