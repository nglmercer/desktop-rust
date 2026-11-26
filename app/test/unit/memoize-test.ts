import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { LruCache, memoize } from '../../src/lib/memoize'

describe('LruCache', () => {
  it('evicts the least recently used entry', () => {
    const cache = new LruCache<string, number>({ maxSize: 2 })
    cache.set('a', 1)
    cache.set('b', 2)
    assert.equal(cache.get('a'), 1)
    cache.set('c', 3)
    assert.equal(cache.get('a'), 1)
    assert.equal(cache.get('b'), undefined)
    assert.equal(cache.get('c'), 3)
  })
})

describe('memoize', () => {
  it('caches by the first primitive argument by default', () => {
    let calls = 0
    const value = memoize((input: number) => {
      calls++
      return input * 2
    })
    assert.equal(value(2), 4)
    assert.equal(value(2), 4)
    assert.equal(calls, 1)
  })

  it('uses a custom cache key and bound receiver', () => {
    let calls = 0
    const receiver = {
      offset: 2,
      value: memoize(
        function (this: { offset: number }, input: { id: number }) {
          calls++
          return input.id + this.offset
        },
        { cacheKey: input => input.id }
      ),
    }
    assert.equal(receiver.value({ id: 2 }), 4)
    assert.equal(receiver.value({ id: 2 }), 4)
    assert.equal(calls, 1)
  })

  it('does not retain rejected promises', async () => {
    let calls = 0
    const value = memoize(async (input: number) => {
      calls++
      if (input === 1) throw new Error('failed')
      return input
    })
    await assert.rejects(value(1))
    await assert.rejects(value(1))
    assert.equal(calls, 2)
  })
})
