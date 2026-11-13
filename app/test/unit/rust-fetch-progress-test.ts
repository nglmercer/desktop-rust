import { describe, expect, it, vi } from 'vitest'
import { FetchStreamProgressParser } from '../../src/lib/progress/fetch-stream'

function chunk(stream: 'stderr' | 'stdout' | 'lfs', value: string | Uint8Array) {
  return { stream, data: Buffer.from(value).toString('base64') }
}

describe('native fetch progress decoding', () => {
  it('decodes split UTF-8 and CR, LF, and CRLF records independently by stream', () => {
    const onProgress = vi.fn()
    const parser = new FetchStreamProgressParser(onProgress)
    const text = Buffer.from('remote: Counting objects: 123 café\r')
    const split = text.indexOf(Buffer.from('é')) + 1
    parser.push(chunk('stderr', text.subarray(0, split)))
    parser.push(chunk('stdout', 'ignored\n'))
    parser.push(chunk('stderr', text.subarray(split)))
    parser.push(chunk('stderr', '\nReceiving objects:  50% (1/2)\rResolving deltas: 100% (2/2), done.\n'))
    expect(onProgress).toHaveBeenCalledTimes(3)
    expect(onProgress.mock.calls[0][0]).toMatchObject({ kind: 'context', text: 'remote: Counting objects: 123 café' })
    expect(onProgress.mock.calls[1][0]).toMatchObject({ kind: 'progress', details: { title: 'Receiving objects', value: 1, total: 2 } })
    expect(onProgress.mock.calls[2][0]).toMatchObject({ kind: 'progress', details: { title: 'Resolving deltas', done: true } })
  })

  it('preserves LFS parsing and suppresses unrelated Git context while LFS is active', () => {
    const onProgress = vi.fn()
    const parser = new FetchStreamProgressParser(onProgress)
    parser.push(chunk('lfs', 'download 1/1 12/'))
    parser.push(chunk('stderr', 'remote: Counting objects: 4\n'))
    parser.push(chunk('lfs', '24 photo.png\n'))
    parser.push(chunk('stderr', 'remote: Counting objects: 5\nReceiving objects:  50% (1/2)\n'))
    expect(onProgress).toHaveBeenCalledTimes(3)
    expect(onProgress.mock.calls[1][0]).toMatchObject({ kind: 'progress', details: { title: 'Downloading "photo.png"', value: 12, total: 24 } })
    expect(onProgress.mock.calls[2][0]).toMatchObject({ kind: 'progress', details: { title: 'Receiving objects' } })
  })

  it('flushes the final line once, ignores late and malformed input, and bounds oversized records', () => {
    const onProgress = vi.fn()
    const parser = new FetchStreamProgressParser(onProgress)
    parser.push({ stream: 'stderr', data: 'bad!' })
    parser.push(chunk('stderr', `${'x'.repeat(1024 * 1024 + 1)}\nremote: Counting objects: 9`))
    parser.finish()
    parser.finish()
    parser.push(chunk('stderr', 'late\n'))
    expect(onProgress).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ kind: 'context', text: 'remote: Counting objects: 9' }))
  })
})
