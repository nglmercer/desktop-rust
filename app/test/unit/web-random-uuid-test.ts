import { describe, it } from 'node:test'
import assert from 'node:assert'
import { randomUUID } from '../../src/web/random-uuid'

describe('randomUUID (webview)', () => {
  it('returns an RFC 4122 version 4 identifier', () => {
    const id = randomUUID()
    assert.match(id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
  })

  it('does not repeat across calls', () => {
    const ids = new Set(Array.from({ length: 1000 }, () => randomUUID()))
    assert.strictEqual(ids.size, 1000)
  })
})
