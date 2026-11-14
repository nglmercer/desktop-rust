import assert from 'node:assert'
import { describe, it, vi } from 'vitest'
import * as React from 'react'

import { CICheckRunNoStepItem } from '../../../src/ui/check-runs/ci-check-run-no-steps'
import { fireEvent, render, screen } from '../../helpers/ui/vitest-render'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({
  TargetPathArgument: '$PATH',
  isValidCustomIntegration: vi.fn(),
}))

describe('small action and dialog surfaces', () => {
  it('renders the no-step check-run state and invokes the external-view callback', () => {
    let externalViewCount = 0

    function onViewCheckExternally() {
      externalViewCount++
    }

    const view = render(
      <CICheckRunNoStepItem onViewCheckExternally={onViewCheckExternally} />
    )

    const button = screen.getByRole('link', { name: 'View check details' })
    const image = view.container.querySelector('.ci-check-run-no-steps img')

    assert.ok(
      screen.getByText('There are no steps to display for this check.', {
        exact: false,
      })
    )
    assert.notEqual(image, null)
    assert.equal(image?.getAttribute('alt'), '')

    fireEvent.click(button)

    assert.equal(externalViewCount, 1)
  })
})
