import assert from 'node:assert'
import { describe, it } from 'vitest'
import { normalizeError } from '../../src/lib/normalize-error'

describe('normalizeError', () => {
  it('preserves string throws without a stack', () => {
    const normalized = normalizeError('boom')
    assert.strictEqual(normalized.name, 'StringError')
    assert.strictEqual(normalized.message, 'boom')
    assert.strictEqual(normalized.stack, undefined)
  })

  it('preserves an Error name, message, and original stack', () => {
    const error = new Error('boom')
    const normalized = normalizeError(error)
    assert.strictEqual(normalized.name, 'Error')
    assert.strictEqual(normalized.message, 'boom')
    assert.strictEqual(normalized.stack, error.stack)
  })

  it('serializes unusual object messages like the legacy reporter', () => {
    assert.strictEqual(normalizeError({ message: null }).message, 'null')
    assert.strictEqual(
      normalizeError({ message: undefined }).message,
      'undefined'
    )
    assert.strictEqual(
      normalizeError({ message: { code: 1 } }).message,
      '{"code":1}'
    )
  })

  it('normalizes non-object throws', () => {
    const normalized = normalizeError(42)
    assert.strictEqual(normalized.name, '[Unknown]')
    assert.strictEqual(normalized.message, '[Unknown]')
    assert.strictEqual(normalized.stack, undefined)
  })
})
