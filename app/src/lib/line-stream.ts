type LineStreamChunk = string | ArrayBufferView

export interface LineStream {
  on(event: 'data', listener: (chunk: LineStreamChunk) => void): unknown
  on(event: 'end', listener: () => void): unknown
}

export function forEachLine(
  stream: LineStream,
  callback: (line: string) => void
): void {
  const decoder = new TextDecoder()
  let pending = ''
  let ended = false

  const consume = (text: string, flush: boolean) => {
    pending += text
    let start = 0
    for (let index = 0; index < pending.length; index++) {
      const character = pending[index]
      if (character !== '\r' && character !== '\n') continue
      if (index > start) callback(pending.slice(start, index))
      start = index + 1
      if (character === '\r' && pending[index + 1] === '\n') {
        index++
        start = index + 1
      }
    }
    pending = pending.slice(start)
    if (flush && pending.length > 0) {
      callback(pending)
      pending = ''
    }
  }

  stream.on('data', chunk => {
    if (ended) return
    const text =
      typeof chunk === 'string'
        ? chunk
        : decoder.decode(chunk, { stream: true })
    consume(text, false)
  })
  stream.on('end', () => {
    if (ended) return
    ended = true
    consume(decoder.decode(), true)
  })
}
