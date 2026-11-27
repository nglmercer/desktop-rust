import assert from 'node:assert'
import { describe, it } from 'vitest'
import { sha256Hex } from '../../src/lib/sha256'

describe('sha256Hex', () => {
  it('matches published SHA-256 vectors', () => {
    assert.strictEqual(
      sha256Hex(''),
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
    )
    assert.strictEqual(
      sha256Hex('abc'),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
    )
    assert.strictEqual(
      sha256Hex('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq'),
      '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1'
    )
  })

  it('hashes text as UTF-8 across block boundaries', () => {
    assert.strictEqual(
      sha256Hex(`${'á'.repeat(40)}📁/${'f'.repeat(80)}`),
      '9e1a6221403b62e5f3b68830d0ce97262b3a9b87152d4ef2218c5bc96b530f41'
    )
  })
})
