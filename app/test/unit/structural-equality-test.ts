import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { structuralEquals } from '../../src/lib/equality'

describe('structuralEquals', () => {
  it('uses strict deep equality for objects, arrays, dates, and regexps', () => {
    assert.equal(
      structuralEquals(
        { a: [1, { b: 'x' }], date: new Date(10), re: /x/gi },
        { date: new Date(10), re: /x/gi, a: [1, { b: 'x' }] }
      ),
      true
    )
    assert.equal(structuralEquals({ a: 1 }, { a: '1' }), false)
    assert.equal(structuralEquals([0], [-0]), false)
    assert.equal(structuralEquals([NaN], [NaN]), true)
    assert.equal(
      structuralEquals({ a: new Date(10) }, { a: new Date(11) }),
      false
    )
    assert.equal(structuralEquals({ a: /x/g }, { a: /x/i }), false)
  })
})
