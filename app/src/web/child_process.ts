// `child_process` for the webview build. Processes are started and driven by
// the Rust shell (`desktop.io.process-*`). Output arrives as `desktop-process`
// events and is routed to the ChildProcess that owns the id, so the surface
// matches Node's: spawn with piped stdio, `close`/`exit` ordering, and
// `exec`/`execFile` built on top of spawn.
import { EventEmitter } from 'events'

type StdioMode = 'pipe' | 'ignore' | 'inherit'

interface IBridge {
  invoke(channel: string, options: object): Promise<any>
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
  public readonly readable = true

  public setEncoding(encoding: string) {
    this.encoding = encoding
    return this
  }

  public emitData(bytes: Uint8Array) {
    const chunk = this.encoding
      ? Buffer.from(bytes).toString(this.encoding as BufferEncoding)
      : Buffer.from(bytes)
    this.emit('data', chunk)
  }

  public pipe<T extends { write(chunk: any): any; end?(): any }>(destination: T): T {
    this.on('data', chunk => destination.write(chunk))
    this.on('end', () => destination.end?.())
    return destination
  }
}

class InputStream extends EventEmitter {
  public readonly writable = true
  private finished = false
  // The SDK writes as soon as it has the stream, which is before `spawn`
  // resolves with the id. Those writes wait here and go out in order.
  private id: number | null = null
  private queued: Array<() => void> = []

  public attach(id: number) {
    this.id = id
    const queued = this.queued.splice(0)
    for (const run of queued) {
      run()
    }
  }

  private whenAttached(run: () => void) {
    if (this.id === null) {
      this.queued.push(run)
    } else {
      run()
    }
  }

  public write(chunk: unknown, encodingOrCallback?: any, callback?: any): boolean {
    const done = typeof encodingOrCallback === 'function' ? encodingOrCallback : callback
    const encoding = typeof encodingOrCallback === 'string' ? encodingOrCallback : undefined
    if (this.finished) {
      const error = new Error('write after end')
      queueMicrotask(() => {
        this.emit('error', error)
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
    this.whenAttached(() => {
      bridge()
        .invoke('desktop.io.process-write', {
          id: this.id,
          data: bridge().encodeBase64(bytes),
        })
        .then(() => done?.(null), (error: Error) => {
          this.emit('error', error)
          done?.(error)
        })
    })
    return true
  }

  public end(chunk?: unknown, callback?: any) {
    if (chunk !== undefined && typeof chunk !== 'function') {
      this.write(chunk)
    }
    if (!this.finished) {
      this.finished = true
      this.whenAttached(() => {
        bridge()
          .invoke('desktop.io.process-end', { id: this.id })
          .then(() => {
            this.emit('finish')
            if (typeof callback === 'function') callback()
          })
      })
    }
    return this
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
    this.stdin = stdio[0] === 'pipe' ? new InputStream(-1) : null
    this.stdout = stdio[1] === 'pipe' ? new OutputStream() : null
    this.stderr = stdio[2] === 'pipe' ? new OutputStream() : null
    this.openStreams = [this.stdout, this.stderr].filter(Boolean).length
  }

  /** @internal */
  public attach(id: number, pid: number | undefined) {
    this.id = id
    this.pid = pid
    this.stdin?.attach(id)
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
    } else if (message.event === 'end') {
      const stream = message.stream === 'stderr' ? this.stderr : this.stdout
      stream?.emit('end')
      this.openStreams -= 1
      this.maybeClose()
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

export function spawn(
  command: string,
  argsOrOptions?: ReadonlyArray<string> | ISpawnOptions,
  maybeOptions?: ISpawnOptions
): ChildProcess {
  const args = Array.isArray(argsOrOptions) ? [...argsOrOptions] : []
  const options: ISpawnOptions =
    (Array.isArray(argsOrOptions) ? maybeOptions : argsOrOptions) ?? {}

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
      ({ id, pid }: { id: number; pid: number }) => {
        child.attach(id, pid)
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
export function exec(
  command: string,
  optionsOrCallback?: Record<string, any> | ((...a: any[]) => void),
  maybeCallback?: (...a: any[]) => void
): ChildProcess {
  const options = typeof optionsOrCallback === 'function' ? undefined : optionsOrCallback
  const callback = typeof optionsOrCallback === 'function' ? optionsOrCallback : maybeCallback
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
