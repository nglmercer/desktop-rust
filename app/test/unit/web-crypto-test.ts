import assert from 'node:assert'
import { describe, it } from 'vitest'
import { createHash, randomBytes, randomUUID } from '../../src/web/crypto'

describe('web crypto compatibility surface', () => {
  it('supports chained SHA-256 updates used by the Copilot SDK', () => {
    assert.strictEqual(
      createHash('sha256').update('ab').update('c').digest('hex'),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
    )
  })

  it('hashes binary input without UTF-8 replacement', () => {
    assert.strictEqual(
      createHash('sha256')
        .update(new Uint8Array([0, 159, 146, 150]))
        .digest('hex'),
      'b02a591131217cb579165aeccf0d94569acffb9934c84d6c813d77e3abedd233'
    )
  })

  it('rejects unsupported algorithms', () => {
    assert.throws(() => createHash('md5'), /Unsupported hash algorithm/)
  })

  it('provides random bytes and identifiers', () => {
    assert.strictEqual(randomBytes(8).length, 8)
    assert.match(
      randomUUID(),
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
    )
  })
})
