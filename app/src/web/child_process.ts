// `child_process` for the webview build. Processes are started and driven by
// the Rust shell (`desktop.io.process-*`). Output arrives as `desktop-process`
// events and is routed to the ChildProcess that owns the id, so the surface
// matches Node's: spawn with piped stdio, `close`/`exit` ordering, and
// `exec`/`execFile` built on top of spawn.
//
// Flow control follows Node's contract in both directions:
//
// * `stdin.write()` returns `false` once a block's worth of data is waiting,
//   and the stream emits `drain` when that data has gone out. Data that is
//   refused is kept here in order, never dropped. A producer that keeps
//   ignoring `false` hits a hard limit: the stream fails explicitly and the
//   child is stopped.
// * Rust returns output credit only when a data block is acknowledged. This
//   side acknowledges after each block has reached its consumer, so a paused
//   consumer stops the child instead of growing a buffer.
import { EventEmitter } from 'events'

type StdioMode = 'pipe' | 'ignore' | 'inherit'

/**
 * Limits Rust reports with each spawn. The defaults only apply in the short
 * window before the id is known; they match the default resource profile.
 */
interface IStreamLimits {
  readonly block: number
  readonly inputBudget: number
  readonly outputWindow: number
}

const DEFAULT_LIMITS: IStreamLimits = {
  block: 64 * 1024,
  inputBudget: 1024 * 1024 - 64 * 1024,
  outputWindow: 256 * 1024,
}

/** Stdin written before the process has an id. Nothing is sent yet, so it is small. */
const PRE_SPAWN_BUDGET = 256 * 1024

interface IBridge {
  invoke(channel: string, options: object): Promise<any>
  send(channel: string, options: object): void
  on(channel: string, listener: (event: unknown, message: any) => void): unknown
  encodeBase64(bytes: Uint8Array): string
  decodeBase64(text: string): Uint8Array
}

const bridge = (): IBridge => (globalThis as any).__desktop

const processChannel = 'desktop-process'
const children = new Map<number, ChildProcess>()
// Output can arrive before `spawn` resolves with the id; hold it until then.
const early = new Map<number, Array<Record<string, any>>>()
let routerInstalled = false

function route(message: Record<string, any>) {
  const child = children.get(message.id)
  if (child === undefined) {
    const queue = early.get(message.id) ?? []
    queue.push(message)
    early.set(message.id, queue)
    return
  }
  child.receive(message)
}

function installRouter() {
  if (routerInstalled) {
    return
  }
  routerInstalled = true
  bridge().on(processChannel, (_event, message) => route(message))
}

/** Node-style error with `code`, `syscall` and `path`, as spawn reports them. */
function spawnError(error: any, command: string, args: string[]) {
  const code = typeof error?.code === 'string' ? error.code : 'EIO'
  return Object.assign(new Error(`spawn ${command} ${code}`, { cause: error }), {
    errno: -2,
    code,
    syscall: `spawn ${command}`,
    path: command,
    spawnargs: args,
  })
}

class OutputStream extends EventEmitter {
  private encoding: string | null = null
  private decoder: TextDecoder | null = null
  public readonly readable = true

  public setEncoding(encoding: string) {
    this.encoding = encoding
    // An incremental decoder keeps a character that is split between two
    // blocks until its last byte arrives.
    this.decoder = /^utf-?8$/i.test(encoding) ? new TextDecoder('utf-8') : null
    return this
  }

  public emitData(bytes: Uint8Array) {
    if (this.encoding === null) {
      this.emit('data', Buffer.from(bytes))
      return
    }
    if (this.decoder !== null) {
      const text = this.decoder.decode(bytes, { stream: true })
      if (text !== '') {
        this.emit('data', text)
      }
      return
    }
    this.emit('data', Buffer.from(bytes).toString(this.encoding as BufferEncoding))
  }

  /** Emit what the incremental decoder still holds at the end of the stream. */
  public finish() {
    if (this.decoder !== null) {
      const text = this.decoder.decode()
      if (text !== '') {
        this.emit('data', text)
      }
    }
  }

  public pipe<T extends { write(chunk: any): any; end?(): any }>(destination: T): T {
    this.on('data', chunk => destination.write(chunk))
    this.on('end', () => destination.end?.())
    return destination
  }
}

interface IQueuedBlock {
  readonly bytes: Uint8Array
  readonly done?: (error?: Error | null) => void
}

class InputStream extends EventEmitter {
  public readonly writable = true
  // Blocks not yet accepted by Rust, in order. Rust holds at most one.
  private readonly queue: IQueuedBlock[] = []
  private queuedBytes = 0
  private id: number | null = null
  private limits: IStreamLimits = DEFAULT_LIMITS
  private inFlight = false
  private waitingForDrain = false
  private wantsDrain = false
  private finished = false
  private endSent = false
  private failure: Error | null = null
  private onFinish: (() => void) | undefined = undefined

  /** `onFailure` stops the child, so a failed stdin never leaves it running. */
  public constructor(private readonly onFailure: () => void) {
    super()
  }

  /** The most input this stream may hold before it refuses to take more. */
  private budget(): number {
    return this.id === null ? PRE_SPAWN_BUDGET : this.limits.inputBudget
  }

  public attach(id: number, limits: IStreamLimits) {
    this.id = id
    this.limits = limits
    this.pump()
  }

  public write(chunk: unknown, encodingOrCallback?: any, callback?: any): boolean {
    const done = typeof encodingOrCallback === 'function' ? encodingOrCallback : callback
    const encoding = typeof encodingOrCallback === 'string' ? encodingOrCallback : undefined
    if (this.finished || this.failure !== null) {
      const error = this.failure ?? new Error('write after end')
      queueMicrotask(() => {
        this.report(error)
        done?.(error)
      })
      return false
    }
    const bytes =
      typeof chunk === 'string'
        ? Buffer.from(chunk, encoding as BufferEncoding)
        : chunk instanceof Uint8Array
        ? chunk
        : Buffer.from(String(chunk))

    if (bytes.length === 0) {
      queueMicrotask(() => done?.(null))
      return this.queuedBytes < this.limits.block
    }
    const block = this.limits.block
    for (let offset = 0; offset < bytes.length; offset += block) {
      const last = offset + block >= bytes.length
      this.queue.push({ bytes: bytes.subarray(offset, offset + block), done: last ? done : undefined })
    }
    this.queuedBytes += bytes.length
    if (this.queuedBytes > this.budget()) {
      this.fail(
        Object.assign(
          new Error(`stdin backlog exceeds ${this.budget()} bytes; the writer ignored write() returning false`),
          { code: 'EOVERFLOW' }
        )
      )
      return false
    }

    this.pump()
    if (this.queuedBytes >= block) {
      this.wantsDrain = true
      return false
    }
    return true
  }

  /** Send the next block, or end-of-input once everything is accepted. */
  private pump() {
    if (this.id === null || this.failure !== null || this.inFlight || this.waitingForDrain) {
      return
    }
    const id = this.id
    const next = this.queue[0]
    if (next === undefined) {
      if (this.finished && !this.endSent) {
        this.sendEnd(id)
      } else if (this.wantsDrain) {
        this.wantsDrain = false
        this.emit('drain')
      }
      return
    }
    this.inFlight = true
    bridge()
      .invoke('desktop.io.process-write', { id, data: bridge().encodeBase64(next.bytes) })
      .then(
        () => {
          if (this.failure !== null) {
            return
          }
          this.inFlight = false
          this.queue.shift()
          this.queuedBytes -= next.bytes.length
          next.done?.(null)
          this.pump()
        },
        (error: any) => {
          if (this.failure !== null) {
            return
          }
          this.inFlight = false
          if (error?.code === 'EBUSY') {
            // Rust still holds the previous block. Its `drain` event resumes us.
            this.waitingForDrain = true
            return
          }
          this.fail(error instanceof Error ? error : new Error(String(error)))
        }
      )
  }

  private sendEnd(id: number) {
    this.endSent = true
    bridge()
      .invoke('desktop.io.process-end', { id })
      .then(
        () => {
          this.emit('finish')
          this.onFinish?.()
        },
        () => {
          // The process is gone; there is no input left to end.
        }
      )
  }

  /** Rust has room again. */
  public resume() {
    this.waitingForDrain = false
    this.pump()
  }

  public end(chunk?: unknown, callback?: any) {
    if (chunk !== undefined && typeof chunk !== 'function') {
      this.write(chunk)
    }
    if (typeof callback === 'function') {
      this.onFinish = callback
    } else if (typeof chunk === 'function') {
      this.onFinish = chunk as () => void
    }
    if (!this.finished) {
      this.finished = true
      this.pump()
    }
    return this
  }

  /**
   * Reject everything still queued. `stop` also ends the child, which is what
   * a failure needs; a kill has already stopped it.
   */
  public fail(error: Error, stop: boolean = true) {
    if (this.failure !== null) {
      return
    }
    this.failure = error
    const queued = this.queue.splice(0)
    this.queuedBytes = 0
    for (const block of queued) {
      block.done?.(error)
    }
    this.report(error)
    if (stop) {
      this.onFailure()
    }
  }

  private report(error: Error) {
    // Node throws on an unhandled 'error'; a stdin failure must not crash the UI.
    if (this.listenerCount('error') > 0) {
      this.emit('error', error)
    }
  }

  public destroy() {
    this.end()
    return this
  }
}

export class ChildProcess extends EventEmitter {
  public pid: number | undefined = undefined
  public exitCode: number | null = null
  public signalCode: string | null = null
  public killed = false
  public readonly stdin: InputStream | null
  public readonly stdout: OutputStream | null
  public readonly stderr: OutputStream | null
  private id = -1
  private exited = false
  private openStreams = 0

  public constructor(stdio: StdioMode[]) {
    super()
    this.stdin = stdio[0] === 'pipe' ? new InputStream(() => this.kill('SIGKILL')) : null
    this.stdout = stdio[1] === 'pipe' ? new OutputStream() : null
    this.stderr = stdio[2] === 'pipe' ? new OutputStream() : null
    this.openStreams = [this.stdout, this.stderr].filter(Boolean).length
  }

  /** @internal */
  public attach(id: number, pid: number | undefined, limits: IStreamLimits = DEFAULT_LIMITS) {
    this.id = id
    this.pid = pid
    this.stdin?.attach(id, limits)
    children.set(id, this)
    for (const message of early.get(id) ?? []) {
      this.receive(message)
    }
    early.delete(id)
  }

  /** @internal */
  public receive(message: Record<string, any>) {
    if (message.event === 'data') {
      const stream = message.stream === 'stderr' ? this.stderr : this.stdout
      stream?.emitData(bridge().decodeBase64(message.data))
      // Credit returns only after the block reached its consumer.
      if (typeof message.seq === 'number') {
        bridge().send('desktop.io.process-ack', { id: this.id, stream: message.stream, seq: message.seq })
      }
    } else if (message.event === 'end') {
      const stream = message.stream === 'stderr' ? this.stderr : this.stdout
      stream?.finish()
      stream?.emit('end')
      this.openStreams -= 1
      this.maybeClose()
    } else if (message.event === 'drain') {
      this.stdin?.resume()
    } else if (message.event === 'exit') {
      this.exited = true
      this.exitCode = message.code ?? null
      this.signalCode = message.signal ?? null
      children.delete(this.id)
      this.emit('exit', this.exitCode, this.signalCode)
      this.maybeClose()
    }
  }

  private maybeClose() {
    if (this.exited && this.openStreams <= 0) {
      this.emit('close', this.exitCode, this.signalCode)
    }
  }

  /**
   * Node's detach knob. The child is already independent of the app's event
   * loop, so there is nothing to change; the methods exist because callers
   * (the external editor launcher) call them.
   */
  public unref() {
    return this
  }

  public ref() {
    return this
  }

  public kill(signal: string = 'SIGTERM') {
    if (this.exited || this.id < 0) {
      return false
    }
    this.killed = true
    // Pending stdin is discarded; a producer waiting on drain gets the error.
    this.stdin?.fail(Object.assign(new Error('the process was killed'), { code: 'EPIPE' }), false)
    bridge().invoke('desktop.io.process-kill', { id: this.id, signal }).catch(() => {})
    return true
  }
}

export interface ISpawnOptions {
  cwd?: string
  env?: Record<string, string | undefined>
  stdio?: StdioMode | StdioMode[]
  shell?: boolean | string
  windowsHide?: boolean
  detached?: boolean
}

function isArgumentList(value: ReadonlyArray<string> | ISpawnOptions | undefined): value is ReadonlyArray<string> {
  return Array.isArray(value)
}

export function spawn(
  command: string,
  argsOrOptions?: ReadonlyArray<string> | ISpawnOptions,
  maybeOptions?: ISpawnOptions
): ChildProcess {
  const args = isArgumentList(argsOrOptions) ? [...argsOrOptions] : []
  const options: ISpawnOptions =
    (isArgumentList(argsOrOptions) ? maybeOptions : argsOrOptions) ?? {}

  const modes: StdioMode[] = Array.isArray(options.stdio)
    ? (['pipe', 'pipe', 'pipe'].map((_, i) => options.stdio![i] ?? 'pipe') as StdioMode[])
    : [options.stdio ?? 'pipe', options.stdio ?? 'pipe', options.stdio ?? 'pipe'].map(
        mode => (mode === 'inherit' ? 'ignore' : mode)
      ) as StdioMode[]

  const env =
    options.env === undefined
      ? undefined
      : Object.fromEntries(
          Object.entries(options.env).map(([key, value]) => [key, value ?? null])
        )

  const child = new ChildProcess(modes)
  installRouter()

  bridge()
    .invoke('desktop.io.process-spawn', {
      command,
      args,
      cwd: options.cwd,
      env,
      stdio: modes,
    })
    .then(
      ({ id, pid, limits }: { id: number; pid: number; limits?: IStreamLimits }) => {
        child.attach(id, pid, limits ?? DEFAULT_LIMITS)
        child.emit('spawn')
      },
      (error: any) => {
        setTimeout(() => child.emit('error', spawnError(error, command, args)), 0)
      }
    )

  return child
}

export interface IExecResult {
  stdout: string
  stderr: string
}

/** Run a file and collect its output, as `execFile` does. */
export function execFile(
  file: string,
  argsOrOptions?: ReadonlyArray<string> | Record<string, any> | ((...a: any[]) => void),
  optionsOrCallback?: Record<string, any> | ((...a: any[]) => void),
  maybeCallback?: (...a: any[]) => void
): ChildProcess {
  const args = Array.isArray(argsOrOptions) ? argsOrOptions : []
  const options = (Array.isArray(argsOrOptions) ? optionsOrCallback : argsOrOptions) as Record<string, any> | undefined
  const callback = [maybeCallback, optionsOrCallback, argsOrOptions].find(
    candidate => typeof candidate === 'function'
  ) as ((error: any, stdout: string, stderr: string) => void) | undefined

  const child = spawn(file, args, {
    cwd: options?.cwd,
    env: options?.env,
    stdio: 'pipe',
  })
  const stdout: string[] = []
  const stderr: string[] = []
  child.stdout?.setEncoding('utf8')
  child.stderr?.setEncoding('utf8')
  child.stdout?.on('data', (chunk: string) => stdout.push(chunk))
  child.stderr?.on('data', (chunk: string) => stderr.push(chunk))
  let reported = false
  const report = (error: any, code?: number | null) => {
    if (reported) return
    reported = true
    const out = stdout.join('')
    const err = stderr.join('')
    if (callback === undefined) return
    if (error) {
      callback(error, out, err)
    } else if (code !== 0 && code !== null && code !== undefined) {
      callback(Object.assign(new Error(`Command failed: ${file} ${args.join(' ')}\n${err}`), { code, stdout: out, stderr: err }), out, err)
    } else {
      callback(null, out, err)
    }
  }
  child.on('error', error => report(error))
  child.on('close', code => report(null, code))
  return child
}

/** Run a command line in the platform shell, as `exec` does. */
function isExecCallback(value: unknown): value is (...args: any[]) => void {
  return typeof value === 'function'
}

export function exec(
  command: string,
  optionsOrCallback?: Record<string, any> | ((...a: any[]) => void),
  maybeCallback?: (...a: any[]) => void
): ChildProcess {
  const options = isExecCallback(optionsOrCallback) ? undefined : optionsOrCallback
  const callback = isExecCallback(optionsOrCallback) ? optionsOrCallback : maybeCallback
  const shell = navigator.userAgent.includes('Windows') ? 'cmd.exe' : '/bin/sh'
  const flags = shell === 'cmd.exe' ? ['/d', '/s', '/c'] : ['-c']
  return execFile(shell, [...flags, command], options, callback)
}

// `util.promisify(execFile)` resolves with `{ stdout, stderr }` in Node, which
// callers such as `which git` rely on. A plain promisify would resolve with
// stdout alone, so the custom promisified form is provided here.
const promisifyCustom = Symbol.for('nodejs.util.promisify.custom')
;(execFile as any)[promisifyCustom] = (file: string, args?: any, options?: any) =>
  new Promise((resolve, reject) => {
    const callback = (error: any, stdout: string, stderr: string) =>
      error ? reject(Object.assign(error, { stdout, stderr })) : resolve({ stdout, stderr })
    if (args === undefined) {
      execFile(file, callback)
    } else if (options === undefined) {
      execFile(file, args, callback)
    } else {
      execFile(file, args, options, callback)
    }
  })

export default { spawn, exec, execFile, ChildProcess }
