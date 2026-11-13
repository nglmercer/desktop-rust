import { describe, it, vi } from 'vitest'
import assert from 'node:assert'

import { getRowOffsetInSection } from '../../src/ui/lib/list/section-list'

// Row offset calculation does not send list-rendering telemetry.
vi.mock('../../src/lib/helpers/non-fatal-exception', () => ({ sendNonFatalException: vi.fn() }))

describe('section-list', () => {
  describe('getRowOffsetInSection', () => {
    it('sums the height of each preceding variable-height row', () => {
      const rowHeights = [30, 46, 30]
      const offset = getRowOffsetInSection(
        ({ index }) => rowHeights[index.row],
        { section: 0, row: 2 }
      )

      assert.strictEqual(offset, 76)
    })
  })
})
