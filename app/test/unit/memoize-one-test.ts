import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { memoizeOne } from '../../src/lib/memoize-one'

describe('memoizeOne', () => {
  it('returns the latest result only for identical arguments and receiver', () => {
    let calls = 0
    const receiver = { offset: 2 }
    const value = memoizeOne(function (
      this: { offset: number },
      input: number
    ) {
      calls++
      return input + this.offset
    })
    assert.equal(value.call(receiver, 2), 4)
    assert.equal(value.call(receiver, 2), 4)
    assert.equal(calls, 1)
    assert.equal(value.call(receiver, 3), 5)
    assert.equal(value.call(receiver, 2), 4)
    assert.equal(calls, 3)
    assert.equal(value.call({ offset: 3 }, 2), 5)
    assert.equal(calls, 4)
  })

  it('passes the argument index to the custom equality function', () => {
    const seen: number[] = []
    const value = memoizeOne(
      (first: object, second: object) => [first, second],
      (_left, _right, index) => {
        seen.push(index)
        return true
      }
    )
    const first = value({}, {})
    assert.strictEqual(value({}, {}), first)
    assert.deepEqual(seen, [0, 1])
  })
})
