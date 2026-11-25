// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { Readable, Writable } from 'node:stream'
import { pipeline } from '../../src/web/stream-promises'

describe('web stream promises', () => {
  it('pipes a readable stream to completion', async () => {
    const chunks: Buffer[] = []
    await pipeline(
      Readable.from(['first-', 'second']),
      new Writable({
        write(chunk, _encoding, callback) {
          chunks.push(Buffer.from(chunk))
          callback()
        },
      })
    )
    expect(Buffer.concat(chunks).toString()).toBe('first-second')
  })

  it('rejects when the source fails', async () => {
    const failure = new Error('source failed')
    await expect(
      pipeline(
        new Readable({
          read() {
            this.destroy(failure)
          },
        }),
        new Writable({
          write(_chunk, _encoding, callback) {
            callback()
          },
        })
      )
    ).rejects.toBe(failure)
  })
})
