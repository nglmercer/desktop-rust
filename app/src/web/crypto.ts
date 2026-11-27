// Minimal `crypto` surface required by the Copilot SDK in the webview. Keep
// this independent from Node crypto polyfills; renderer code should use the
// purpose-specific helpers in lib instead of importing this compatibility module.

import { sha256BytesHex } from '../lib/sha256'
import { randomUUID } from './random-uuid'
import './install-random-uuid'

export { randomUUID }

class Sha256Hash {
  private readonly chunks: Uint8Array[] = []
  private digested = false

  public update(value: string | Uint8Array): this {
    if (this.digested) {
      throw new Error('Hash already digested')
    }
    this.chunks.push(
      typeof value === 'string' ? new TextEncoder().encode(value) : value
    )
    return this
  }

  public digest(encoding: 'hex'): string {
    if (encoding !== 'hex') {
      throw new Error(`Unsupported digest encoding: ${encoding}`)
    }
    this.digested = true
    const length = this.chunks.reduce((sum, chunk) => sum + chunk.length, 0)
    const bytes = new Uint8Array(length)
    let offset = 0
    for (const chunk of this.chunks) {
      bytes.set(chunk, offset)
      offset += chunk.length
    }
    return sha256BytesHex(bytes)
  }
}

export function createHash(algorithm: string): Sha256Hash {
  if (algorithm !== 'sha256') {
    throw new Error(`Unsupported hash algorithm: ${algorithm}`)
  }
  return new Sha256Hash()
}

export function randomBytes(length: number): Uint8Array {
  if (!Number.isSafeInteger(length) || length < 0) {
    throw new RangeError('Invalid random byte length')
  }
  const bytes = new Uint8Array(length)
  crypto.getRandomValues(bytes)
  return bytes
}
