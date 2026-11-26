import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { compareSystemVersions } from '../../src/lib/get-os'

describe('compareSystemVersions', () => {
  it('compares numeric components rather than strings', () => {
    assert.equal(compareSystemVersions('10.0.17666', '10.0.9999'), 1)
    assert.equal(compareSystemVersions('10.15.7', '10.16'), -1)
  })

  it('pads missing components with zeroes', () => {
    assert.equal(compareSystemVersions('26.0.0', '26'), 0)
    assert.equal(compareSystemVersions('14.0', '14.0.1'), -1)
  })

  it('rejects non-numeric versions', () => {
    assert.equal(compareSystemVersions('26.0.0-beta', '26'), undefined)
    assert.equal(compareSystemVersions('26', 'unknown'), undefined)
  })
})
