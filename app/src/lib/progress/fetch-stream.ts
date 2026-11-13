import type { FetchProgressChunk } from '../../backend/client'
import { FetchProgressParser } from './fetch'
import type { IGitOutput, IGitProgress } from './git'
import { GitLFSProgressParser } from './lfs'

type Stream = 'stderr' | 'lfs'

interface StreamState {
  readonly decoder: TextDecoder
  line: string
  afterCarriageReturn: boolean
  droppingLine: boolean
}

// Git and LFS records are much shorter than this. Drop an oversized record
// until its delimiter so untrusted remote output cannot grow memory forever.
const maximumLineLength = 1024 * 1024

/** Parse the request-owned bytes from one native fetch operation. */
export class FetchStreamProgressParser {
  private readonly git = new FetchProgressParser()
  private readonly lfs = new GitLFSProgressParser()
  private readonly streams: Record<Stream, StreamState> = {
    stderr: this.newState(),
    lfs: this.newState(),
  }
  private lfsActive = false
  private closed = false

  public constructor(
    private readonly onProgress: (progress: IGitProgress | IGitOutput) => void
  ) {}

  private newState(): StreamState {
    return {
      decoder: new TextDecoder(),
      line: '',
      afterCarriageReturn: false,
      droppingLine: false,
    }
  }

  public push(chunk: FetchProgressChunk): void {
    if (this.closed || chunk.stream === 'stdout') return
    let bytes: Uint8Array
    try {
      const binary = atob(chunk.data)
      bytes = Uint8Array.from(binary, character => character.charCodeAt(0))
    } catch {
      return
    }
    const state = this.streams[chunk.stream]
    this.consume(chunk.stream, state.decoder.decode(bytes, { stream: true }))
  }

  /** Flush a final record even if Git exits without a newline. */
  public finish(): void {
    if (this.closed) return
    for (const stream of ['stderr', 'lfs'] as const) {
      const state = this.streams[stream]
      this.consume(stream, state.decoder.decode())
      if (state.line && !state.droppingLine) this.emit(stream, state.line)
      state.line = ''
    }
    this.closed = true
  }

  private consume(stream: Stream, text: string): void {
    const state = this.streams[stream]
    for (const character of text) {
      if (state.afterCarriageReturn) {
        state.afterCarriageReturn = false
        if (character === '\n') continue
      }
      if (character === '\r' || character === '\n') {
        if (state.line && !state.droppingLine) this.emit(stream, state.line)
        state.line = ''
        state.droppingLine = false
        state.afterCarriageReturn = character === '\r'
      } else if (!state.droppingLine) {
        state.line += character
        if (state.line.length > maximumLineLength) {
          state.line = ''
          state.droppingLine = true
        }
      }
    }
  }

  private emit(stream: Stream, line: string): void {
    if (stream === 'lfs') {
      const progress = this.lfs.parse(line)
      if (progress.kind === 'progress') {
        this.lfsActive = true
        this.onProgress(progress)
      }
      return
    }
    const progress = this.git.parse(line)
    if (this.lfsActive) {
      if (progress.kind === 'context') return
      if (progress.details.title === 'Filtering content') {
        if (progress.details.done) this.lfsActive = false
        return
      }
    }
    this.onProgress(progress)
  }
}
