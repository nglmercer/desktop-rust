// `dugite` for the webview build. git itself is run by the Rust shell
// (`desktop.io.git-exec`); the error tables and parsers are dugite's own.
import { bundleUrl } from './bundle'
// @ts-ignore -- aliased to dugite's build output in vite.config.ts
import * as errors from 'desktop-dugite/errors'
// @ts-ignore
import * as parseErrorModule from 'desktop-dugite/parse-error'
// @ts-ignore
import * as badConfigModule from 'desktop-dugite/parse-bad-config-value-error-info'

export const GitError = errors.GitError
export const GitErrorRegexes = errors.GitErrorRegexes
export const ExecError: new (
  message: string,
  stdout: Buffer | string,
  stderr: Buffer | string,
  cause?: unknown
) => Error & { code?: string; stdout: Buffer | string; stderr: Buffer | string } =
  errors.ExecError
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
  readonly processCallback?: (process: unknown) => void
  readonly signal?: AbortSignal
  readonly killSignal?: string | number
}
export type IGitStringExecutionOptions = IGitExecutionOptions
export type IGitBufferExecutionOptions = IGitExecutionOptions
export type IGitStringResult = IGitResult & { stdout: string; stderr: string }
export type IGitBufferResult = IGitResult & { stdout: Buffer; stderr: Buffer }
export type IGitSpawnOptions = { readonly env?: Record<string, string | undefined> }

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
const newCancelId = () => Math.floor(Math.random() * 2 ** 52) + 1

/**
 * Run git and resolve with its output whatever the exit code, like
 * `dugite.exec`. `processCallback` is not called: there is no child process
 * object on this side of the bridge, so output arrives only when git exits.
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

export function spawn(): never {
  throw new Error(
    'Streaming git processes are not available in the Rust shell yet'
  )
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
