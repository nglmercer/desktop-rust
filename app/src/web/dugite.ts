// `dugite` for the webview build. git itself is run by the Rust shell
// (`desktop.io.git-exec`); the error tables and parsers are dugite's own.
import { bundleUrl } from './bundle'
import { spawn as spawnProcess, type ChildProcess } from './child_process'
// @ts-ignore -- aliased to dugite's build output in vite.config.ts
import * as errors from 'desktop-dugite/errors'
// @ts-ignore
import * as parseErrorModule from 'desktop-dugite/parse-error'
// @ts-ignore
import * as badConfigModule from 'desktop-dugite/parse-bad-config-value-error-info'

export class ExecError extends Error {
  readonly code?: string
  readonly signal?: string
  readonly killed?: boolean
  readonly stdout: Buffer | string
  readonly stderr: Buffer | string

  constructor(
    message: string,
    stdout: Buffer | string,
    stderr: Buffer | string,
    cause?: unknown
  ) {
    super(message, { cause })
    this.stdout = stdout
    this.stderr = stderr
    if (cause !== null && typeof cause === 'object') {
      const source = cause as Record<string, unknown>
      if (typeof source.code === 'string') this.code = source.code
      if (typeof source.signal === 'string') this.signal = source.signal
      if (typeof source.killed === 'boolean') this.killed = source.killed
    }
  }
}

export const GitError = errors.GitError
export const GitErrorRegexes = errors.GitErrorRegexes
export const parseError = parseErrorModule.parseError
export const parseBadConfigValueErrorInfo =
  badConfigModule.parseBadConfigValueErrorInfo

export interface IGitResult {
  readonly stdout: string | Buffer
  readonly stderr: string | Buffer
  readonly exitCode: number
}
export interface IGitExecutionOptions {
  readonly env?: Record<string, string | undefined>
  readonly stdin?: string | Buffer
  readonly stdinEncoding?: BufferEncoding
  readonly encoding?: BufferEncoding | 'buffer'
  readonly maxBuffer?: number
  readonly processCallback?: (process: ChildProcess) => void
  readonly signal?: AbortSignal
  readonly killSignal?: string | number
}
export type IGitStringExecutionOptions = IGitExecutionOptions
export type IGitBufferExecutionOptions = IGitExecutionOptions
export type IGitStringResult = IGitResult & { stdout: string; stderr: string }
export type IGitBufferResult = IGitResult & { stdout: Buffer; stderr: Buffer }
export type IGitSpawnOptions = {
  readonly env?: Record<string, string | undefined>
}

interface IBridge {
  invoke(channel: string, options: object): Promise<any>
  send(channel: string, options: object): void
  encodeBase64(bytes: Uint8Array): string
  decodeBase64(text: string): Uint8Array
}

const bridge = (): IBridge => (globalThis as any).__desktop

const abortError = (signal: AbortSignal) =>
  signal.reason ?? new DOMException('The operation was aborted.', 'AbortError')

/**
 * Typed data Rust attaches to some failures (see `RpcError.details`). For a
 * command stopped at `maxBuffer`, `stdout` and `stderr` hold what it produced
 * before it was stopped, encoded as the request asked (base64 for buffers).
 */
interface IGitErrorDetails {
  readonly stdout?: string
  readonly stderr?: string
  readonly truncated?: boolean
}

/**
 * Ids are random so that a cancellation left over from an earlier page load
 * cannot stop a new command that happens to reuse the number.
 */
const newCancelId = () => {
  const words = new Uint32Array(2)
  globalThis.crypto.getRandomValues(words)
  // A 53-bit number: the high word keeps 21 bits, so the value stays exact.
  return (words[0] % 2 ** 21) * 2 ** 32 + words[1] + 1
}

function outputLength(chunk: string | Buffer): number {
  return typeof chunk === 'string' ? Buffer.byteLength(chunk) : chunk.length
}

function joinOutput(chunks: ReadonlyArray<string | Buffer>, binary: boolean) {
  return binary
    ? Buffer.concat(chunks.map(chunk => Buffer.from(chunk as Buffer)))
    : chunks.join('')
}

/**
 * Interactive commands need the live child object that dugite exposes through
 * processCallback. Drive the host's streaming process implementation while
 * preserving execFile-style output collection and maxBuffer failure.
 */
function execInteractive(
  args: string[],
  path: string,
  options: IGitExecutionOptions
): Promise<any> {
  return new Promise((resolve, reject) => {
    const binary = options.encoding === 'buffer'
    const cwd = bundleUrl(path) === null ? path : undefined
    const child = spawnProcess('git', args, {
      cwd,
      env: options.env,
      signal: options.signal,
      useGitBinary: true,
      stdio: 'pipe',
    })
    const stdout: Array<string | Buffer> = []
    const stderr: Array<string | Buffer> = []
    let stdoutLength = 0
    let stderrLength = 0
    let truncated = false
    const maxBuffer =
      options.maxBuffer !== undefined && Number.isFinite(options.maxBuffer)
        ? options.maxBuffer
        : Infinity

    const collect = (
      chunks: Array<string | Buffer>,
      chunk: string | Buffer,
      stdoutStream: boolean
    ) => {
      chunks.push(chunk)
      if (stdoutStream) {
        stdoutLength += outputLength(chunk)
      } else {
        stderrLength += outputLength(chunk)
      }
      if (!truncated && Math.max(stdoutLength, stderrLength) > maxBuffer) {
        truncated = true
        child.kill(String(options.killSignal ?? 'SIGTERM'))
      }
    }

    if (!binary) {
      child.stdout?.setEncoding(options.encoding ?? 'utf8')
      child.stderr?.setEncoding(options.encoding ?? 'utf8')
    }
    child.stdout?.on('data', chunk => collect(stdout, chunk, true))
    child.stderr?.on('data', chunk => collect(stderr, chunk, false))
    child.on('error', error => {
      reject(
        new ExecError(
          error.message,
          joinOutput(stdout, binary),
          joinOutput(stderr, binary),
          error
        )
      )
    })
    child.on('close', () => {
      const stdoutOutput = joinOutput(stdout, binary)
      const stderrOutput = joinOutput(stderr, binary)
      if (truncated) {
        const cause = Object.assign(new Error('maxBuffer exceeded'), {
          code: 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER',
          killed: true,
          signal: child.signalCode ?? undefined,
        })
        reject(
          new ExecError('maxBuffer exceeded', stdoutOutput, stderrOutput, cause)
        )
        return
      }
      if (child.exitCode === null) {
        const signal = child.signalCode ?? 'unknown signal'
        const cause = Object.assign(
          new Error(`Command was killed with ${signal}`),
          {
            code: null,
            signal: child.signalCode ?? undefined,
            killed: child.killed,
          }
        )
        reject(
          new ExecError(
            `Command was killed with ${signal}`,
            stdoutOutput,
            stderrOutput,
            cause
          )
        )
        return
      }
      resolve({
        stdout: stdoutOutput,
        stderr: stderrOutput,
        exitCode: child.exitCode,
      })
    })

    if (options.stdin !== undefined && child.stdin !== null) {
      if (options.stdinEncoding !== undefined) {
        child.stdin.end(options.stdin as string, options.stdinEncoding)
      } else {
        child.stdin.end(options.stdin)
      }
    }
    try {
      options.processCallback?.(child)
    } catch (error) {
      child.kill()
      reject(error)
    }
  })
}

/**
 * Run git and resolve with its output whatever the exit code, like
 * `dugite.exec`. Commands that need the live child use the streaming host
 * process; the rest use the bounded one-shot host operation.
 */
export async function exec(
  args: string[],
  path: string,
  options?: IGitExecutionOptions
): Promise<any> {
  const signal = options?.signal
  if (signal?.aborted) {
    throw abortError(signal)
  }
  if (options?.processCallback !== undefined) {
    return execInteractive(args, path, options)
  }

  const binary = options?.encoding === 'buffer'
  const stdin =
    options?.stdin === undefined
      ? undefined
      : bridge().encodeBase64(
          typeof options.stdin === 'string'
            ? Buffer.from(options.stdin, options.stdinEncoding ?? 'utf8')
            : options.stdin
        )

  // JSON has no `undefined`; null tells Rust to unset the variable.
  const env: Record<string, string | null> = {}
  for (const [key, value] of Object.entries(options?.env ?? {})) {
    env[key] = value === undefined ? null : String(value)
  }

  // Only a command that can be cancelled gets an id; Rust stops it when the
  // signal fires, or refuses to start it if the signal fired while it queued.
  const cancelId = signal === undefined ? undefined : newCancelId()

  const emptyOutput = () => (binary ? Buffer.alloc(0) : '')
  const run = bridge()
    .invoke('desktop.io.git-exec', {
      args,
      // Commands that are not about a repository run "next to the app".
      // That directory does not exist here; Rust then uses its own.
      cwd: bundleUrl(path) === null ? path : '',
      env,
      stdin,
      encoding: binary ? 'buffer' : 'utf8',
      maxBuffer:
        options?.maxBuffer !== undefined && Number.isFinite(options.maxBuffer)
          ? options.maxBuffer
          : undefined,
      cancelId,
    })
    .then(
      result => {
        const decode = (value: string) =>
          binary ? Buffer.from(bridge().decodeBase64(value)) : value
        return {
          stdout: decode(result.stdout),
          stderr: decode(result.stderr),
          exitCode: result.exitCode as number,
        }
      },
      (error: Error & { code?: string; details?: IGitErrorDetails }) => {
        const message =
          error.code === 'ENOENT'
            ? 'ENOENT: Git failed to execute. This typically means that ' +
              `the path provided doesn't exist or that the Git executable ` +
              `could not be found. Path: ${path}`
            : error.message
        const cause = Object.assign(new Error(error.message), {
          code: error.code,
        })
        // Keep whatever output arrived before the command was stopped. Without
        // it, a preview cut at `maxBuffer` would look like an empty file.
        const partial = (value: string | undefined) =>
          value === undefined || value === ''
            ? emptyOutput()
            : binary
              ? Buffer.from(bridge().decodeBase64(value))
              : value
        const details = error.details
        throw new ExecError(
          message,
          partial(details?.stdout),
          partial(details?.stderr),
          cause
        )
      }
    )

  if (signal === undefined || cancelId === undefined) {
    return run
  }
  // Stop the git process as well as no longer waiting for it.
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      bridge().send('desktop.io.cancel', { cancelId })
      reject(abortError(signal))
    }
    signal.addEventListener('abort', onAbort, { once: true })
    run
      .then(resolve, reject)
      .finally(() => signal.removeEventListener('abort', onAbort))
  })
}

export function spawn(
  args: string[],
  path: string,
  options?: IGitSpawnOptions
): ChildProcess {
  return spawnProcess('git', args, {
    cwd: bundleUrl(path) === null ? path : undefined,
    env: options?.env,
    useGitBinary: true,
    stdio: 'pipe',
  })
}

// The Rust shell chooses the git binary (DESKTOP_GIT_PATH or `git` on PATH).
export const resolveGitBinary = () => 'git'
export const resolveGitDir = () => ''
export const resolveEmbeddedGitDir = () => ''
export const resolveGitExecPath = () => ''
export function setupEnvironment(
  environmentVariables: Record<string, string | undefined>
) {
  return { env: { ...environmentVariables }, gitLocation: 'git' }
}
export const ignoreClosedInputStream = () => {}
