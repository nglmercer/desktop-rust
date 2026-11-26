import assert from 'node:assert/strict'
import { PassThrough } from 'node:stream'
import { describe, it } from 'vitest'
import { forEachLine, forEachNullRecord } from '../../src/lib/line-stream'

describe('forEachLine', () => {
  it('emits non-empty CR, LF, and CRLF records', async () => {
    const stream = new PassThrough()
    const lines: string[] = []
    forEachLine(stream, line => lines.push(line))
    const ended = new Promise(resolve => stream.on('end', resolve))
    stream.end('first\rsecond\nthird\r\n\nfourth')
    await ended
    assert.deepEqual(lines, ['first', 'second', 'third', 'fourth'])
  })

  it('preserves multibyte characters split across chunks', async () => {
    const stream = new PassThrough()
    const lines: string[] = []
    forEachLine(stream, line => lines.push(line))
    const bytes = new TextEncoder().encode('café\n')
    const ended = new Promise(resolve => stream.on('end', resolve))
    stream.write(bytes.subarray(0, 4))
    stream.end(bytes.subarray(4))
    await ended
    assert.deepEqual(lines, ['café'])
  })
  it('splits NUL records while preserving empty records', async () => {
    const stream = new PassThrough()
    const records: string[] = []
    forEachNullRecord(stream, record => records.push(record))
    const ended = new Promise(resolve => stream.on('end', resolve))
    stream.end('\0first\0\0last')
    await ended
    assert.deepEqual(records, ['', 'first', '', 'last'])
  })

  it('preserves multibyte NUL records split across chunks', async () => {
    const stream = new PassThrough()
    const records: string[] = []
    forEachNullRecord(stream, record => records.push(record))
    const bytes = new TextEncoder().encode('café\0')
    const ended = new Promise(resolve => stream.on('end', resolve))
    stream.write(bytes.subarray(0, 4))
    stream.end(bytes.subarray(4))
    await ended
    assert.deepEqual(records, ['café'])
  })
})
