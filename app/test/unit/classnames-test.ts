import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { classNames } from '../../src/lib/classnames'

describe('classNames', () => {
  it('combines strings, numbers, arrays, and truthy object keys', () => {
    assert.equal(
      classNames('base', ['nested', ['deep']], 0, 2, {
        active: true,
        disabled: false,
        missing: undefined,
      }),
      'base nested deep 2 active'
    )
  })

  it('supports object string conversion like the package it replaces', () => {
    assert.equal(classNames({ toString: () => 'custom' }), 'custom')
  })
})
