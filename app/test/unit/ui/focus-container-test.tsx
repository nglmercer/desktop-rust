import assert from 'node:assert'
import { it, vi, afterEach } from 'vitest'
import * as React from 'react'
import { FocusContainer } from '../../../src/ui/lib/focus-container'
import { fireEvent, render, screen } from '../../helpers/ui/vitest-render'

afterEach(() => vi.restoreAllMocks())

it('cancels deferred focus notifications before children unmount', () => {
  const queued = vi.spyOn(globalThis, 'requestAnimationFrame').mockImplementation(() => 42)
  const canceled = vi.spyOn(globalThis, 'cancelAnimationFrame').mockImplementation(() => {})
  let notifications = 0
  const view = render(
    <FocusContainer onFocusWithinChanged={() => notifications++}>
      <button>Focus me</button>
    </FocusContainer>
  )

  fireEvent.focusIn(screen.getByRole('button', { name: 'Focus me' }))
  assert.strictEqual(queued.mock.calls.length, 1)
  view.unmount()
  assert.deepStrictEqual(
    canceled.mock.calls,
    [[42]]
  )
  assert.strictEqual(notifications, 0)
})
