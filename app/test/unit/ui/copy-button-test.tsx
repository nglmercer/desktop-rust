import assert from 'node:assert'
import { afterEach, beforeEach, describe, it, vi } from 'vitest'
import * as React from 'react'
import { act } from 'react-dom/test-utils'

import { fireEvent, render, screen } from '../../helpers/ui/vitest-render'
import { captureClipboardWrites } from '../../helpers/ui/electron'
import { CopyButton } from '../../../src/ui/copy-button'

describe('CopyButton', () => {
  let clipboardCapture: ReturnType<typeof captureClipboardWrites>

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] })
    clipboardCapture = captureClipboardWrites()
  })

  afterEach(() => {
    clipboardCapture.restore()
    vi.useRealTimers()
  })

  it('copies content and announces the copied state before resetting', async () => {
    render(
      <CopyButton copyContent="refs/heads/main" ariaLabel="Copy branch name" />
    )

    const button = screen.getByRole('button', { name: 'Copy branch name' })

    await act(async () => {
      fireEvent.click(button)
    })

    assert.deepEqual(clipboardCapture.writes, ['refs/heads/main'])

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000)
    })
    const liveRegion = screen.getByText(/^Copied!/, {
      selector: '[aria-live="polite"]',
    })
    assert.ok(liveRegion.textContent?.startsWith('Copied!'))

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    assert.equal(
      screen.queryByText(/^Copied!/, { selector: '[aria-live="polite"]' }),
      null
    )
  })
})
