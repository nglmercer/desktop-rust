import { afterEach, describe, it, vi } from 'vitest'
import assert from 'node:assert'
import { randomUUID } from '../../src/web/random-uuid'

describe('randomUUID (webview)', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('installs a fallback when the webview lacks randomUUID', async () => {
    const crypto = { getRandomValues: (bytes: Uint8Array) => bytes.fill(0) }
    vi.stubGlobal('crypto', crypto)
    vi.resetModules()
    await import('../../src/web/install-random-uuid')
    assert.strictEqual(
      globalThis.crypto.randomUUID(),
      '00000000-0000-4000-8000-000000000000'
    )
  })

  it('preserves an existing native randomUUID', async () => {
    const native = () => 'native-uuid'
    vi.stubGlobal('crypto', { randomUUID: native })
    vi.resetModules()
    await import('../../src/web/install-random-uuid')
    assert.strictEqual(globalThis.crypto.randomUUID, native)
  })

  it('returns an RFC 4122 version 4 identifier', () => {
    const id = randomUUID()
    assert.match(
      id,
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
    )
  })

  it('does not repeat across calls', () => {
    const ids = new Set(Array.from({ length: 1000 }, () => randomUUID()))
    assert.strictEqual(ids.size, 1000)
  })
})
