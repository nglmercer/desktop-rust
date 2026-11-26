export interface IChildProcessStream {
  on(event: 'data', listener: (chunk: any) => void): unknown
  off(event: 'data', listener: (chunk: any) => void): unknown
  on(event: 'end', listener: () => void): unknown
}

export interface IChildProcess {
  readonly stdout: IChildProcessStream | null
  readonly stderr: IChildProcessStream | null
  readonly stdin: {
    write(chunk: string): unknown
    end(chunk?: string): unknown
  } | null
  readonly exitCode: number | null
  readonly signalCode: string | null
  on(event: 'error', listener: (error: Error) => void): this
  on(
    event: 'exit',
    listener: (code: number | null, signal: string | null) => void
  ): this
  on(
    event: 'close',
    listener: (code: number | null, signal: string | null) => void
  ): this
  once(event: 'exit', listener: () => void): this
}
