import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { afterAll, beforeAll, vi } from 'vitest'
import {
  backend,
  BackendClient,
  overrideBackendForTests,
} from '../../src/backend/client'

/**
 * A real Rust bridge for tests: it starts `desktop-rs serve`, attaches a
 * WebSocket `BackendClient` the way the browser renderer does, and tears both
 * down. Tests that route `backend()` to `harness.client` exercise the actual
 * Rust operations instead of the Node-backed fallbacks.
 *
 * The server binary comes from `cargo build -p desktop-cli`; use
 * `rustBackendAvailable` to skip when it has not been built.
 */
export const rustBackendBinary = path.resolve('rust/target/debug/desktop-rs')
export const rustBackendAvailable =
  process.platform !== 'win32' && existsSync(rustBackendBinary)

interface IDom {
  readonly window: Window & typeof globalThis & { close(): void }
}

export interface IRustBackend {
  readonly client: BackendClient
  /** An empty temporary directory removed by `stop()`. */
  readonly directory: string
  stop(): Promise<void>
}

async function bounded<T>(promise: Promise<T>, milliseconds = 15_000) {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Rust backend did not respond in time')),
          milliseconds
        )
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

/**
 * The Git settings a test changes in its own process to set up a scenario
 * (ownership checks, tracing, an isolated home). The server is another process,
 * so it has to be restarted with them to see the same thing.
 */
const GIT_ENVIRONMENT_KEYS = [
  'HOME',
  'XDG_CONFIG_HOME',
  'GIT_CONFIG_GLOBAL',
  'GIT_CONFIG_SYSTEM',
  'GIT_CONFIG_NOSYSTEM',
  'GIT_ALTERNATE_OBJECT_DIRECTORIES',
  'GIT_ATTR_SOURCE',
  'GIT_TEST_ASSUME_DIFFERENT_OWNER',
  'GIT_TRACE',
  'GIT_TRACE2',
  'GIT_TRACE2_EVENT',
  'GIT_TRACE2_PERF',
] as const

const gitEnvironmentFingerprint = () =>
  JSON.stringify(GIT_ENVIRONMENT_KEYS.map(key => process.env[key] ?? null))

interface ISession {
  readonly client: BackendClient
  readonly directory: string
  /** The Git environment the server was started with. */
  readonly fingerprint: string
  /** Unwrapped request of `client`, for forwarding after a restart. */
  readonly rawRequest: BackendClient['request']
  close(): Promise<void>
}

async function openSession(directory: string): Promise<ISession> {
  // Keep the caller's Git settings out of the server's environment, except the
  // author and committer the test environment defines for its commits and the
  // settings a test has changed for its scenario.
  const env: Record<string, string> = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key, value]) =>
        value !== undefined &&
        (!key.startsWith('GIT_') ||
          /^GIT_(AUTHOR|COMMITTER)_(NAME|EMAIL)$/.test(key))
    ) as Array<[string, string]>
  )
  const fingerprint = gitEnvironmentFingerprint()
  const overlay = GIT_ENVIRONMENT_KEYS.filter(key => key in process.env)
  const changedByTest = overlay.some(
    key => process.env[key] !== undefined && key !== 'HOME'
  )
  const server: ChildProcess = spawn(
    rustBackendBinary,
    [
      '--config',
      path.join(directory, 'config.json'),
      'serve',
      '--empty',
      '--port',
      '0',
    ],
    {
      env: {
        ...env,
        ...(changedByTest
          ? Object.fromEntries(
              overlay.flatMap(key =>
                process.env[key] === undefined
                  ? []
                  : [[key, process.env[key] as string]]
              )
            )
          : { HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }),
      },
      stdio: ['ignore', 'ignore', 'pipe'],
    }
  )
  const lines = createInterface({ input: server.stderr! })
  let url: string
  try {
    const line = await bounded(
      new Promise<string>((resolve, reject) => {
        lines.once('line', resolve)
        server.once('error', reject)
        server.once('exit', code =>
          reject(new Error(`bridge exited before startup: ${code}`))
        )
      })
    )
    url = line.startsWith('Open ') ? line.slice(5) : ''
  } catch (error) {
    server.kill('SIGKILL')
    throw error
  }

  const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
    JSDOM: new (html: string, options: { url: string }) => IDom
  }
  const dom = new JSDOM('', { url })
  // UI suites render components that read these from `window`; give the
  // bridge's window the stubs the test setup installs on the shared one.
  const shared = globalThis as unknown as {
    ResizeObserver?: unknown
  }
  Object.assign(dom.window, { ResizeObserver: shared.ResizeObserver })
  dom.window.Element.prototype.getClientRects = function () {
    return [
      { x: 0, y: 0, width: 1, height: 1, top: 0, left: 0, right: 1, bottom: 1 },
    ] as unknown as DOMRectList
  }
  vi.stubGlobal('window', dom.window)
  vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
  vi.stubGlobal('WebSocket', dom.window.WebSocket)
  vi.stubGlobal('location', dom.window.location)
  const client = new BackendClient()
  const rawRequest = client.request.bind(client)
  await bounded(client.waitReady())

  return {
    client,
    directory,
    fingerprint,
    rawRequest,
    async close() {
      client.close()
      dom.window.close()
      if (server.exitCode === null && server.signalCode === null) {
        const exited = new Promise<void>(resolve =>
          server.once('exit', () => resolve())
        )
        server.kill('SIGINT')
        try {
          await bounded(exited, 5_000)
        } catch {
          server.kill('SIGKILL')
        }
      }
    },
  }
}

export async function startRustBackend(): Promise<IRustBackend> {
  if (!rustBackendAvailable) {
    throw new Error(`Build the bridge first: ${rustBackendBinary} is missing`)
  }
  const directory = await mkdtemp(path.join(tmpdir(), 'desktop-rust-backend-'))
  let active: ISession
  try {
    active = await openSession(directory)
  } catch (error) {
    await rm(directory, { recursive: true, force: true })
    throw error
  }
  // Every session's client follows the repository a call is about, and when
  // the test changed the Git environment, forwards to a restarted session. The
  // first session's client is the stable handle tests hold; a restarted
  // session's client is installed as `backend()` with the same behavior.
  const handle = active.client
  let selected: string | null = null
  const install = (session: ISession) => {
    const original = session.rawRequest
    session.client.request = async (
      method: string,
      params: unknown,
      opts?: any
    ) => {
      if (gitEnvironmentFingerprint() !== active.fingerprint) {
        const previous = active
        const installRestarted = [session.client, active.client].includes(
          backend() as BackendClient
        )
        active = await openSession(directory)
        install(active)
        if (installRestarted) {
          overrideBackendForTests(active.client)
        }
        selected = null
        await previous.close()
        return active.client.request(method, params, opts)
      }
      const target = (params as { path?: unknown } | null)?.path
      if (
        method === 'repository_binding' &&
        typeof target === 'string' &&
        target !== selected
      ) {
        await active.client.selectBrowserRepository(target)
        selected = target
      }
      return (active === session ? original : active.rawRequest)(
        method,
        params,
        opts
      )
    }
  }
  install(active)

  return {
    client: handle,
    directory,
    async stop() {
      vi.unstubAllGlobals()
      await active.close()
      await rm(directory, { recursive: true, force: true })
    },
  }
}

/**
 * Route `backend()` to a real Rust server for every test in the file.
 *
 * Call it at the top of a test file (inside `describe` or at module level). The
 * server starts once before the file's tests and stops after them; the returned
 * object is filled in when it is ready, so read `rust.client` and
 * `rust.directory` inside tests, not at definition time.
 */
export function useRustBackend(): { client: BackendClient; directory: string } {
  const handle = {} as { client: BackendClient; directory: string }
  let started: IRustBackend | undefined
  beforeAll(async () => {
    started = await startRustBackend()
    Object.assign(handle, {
      client: started.client,
      directory: started.directory,
    })
    overrideBackendForTests(started.client)
  }, 60_000)
  afterAll(async () => {
    overrideBackendForTests(null)
    await started?.stop()
  }, 60_000)
  return handle
}

/**
 * The head and branch the app has just read when it starts an operation. The
 * Rust operations take them as the basis the user acted on and refuse to run
 * if the repository moved; tests that call the library directly supply them
 * from the repository as it is.
 */
export async function currentBasis(repository: {
  readonly path: string
}): Promise<{ readonly head: string; readonly branch: string | null }> {
  const { exec } = await import('dugite')
  const head = (
    await exec(['rev-parse', 'HEAD'], repository.path)
  ).stdout.trim()
  const symbolic = await exec(['symbolic-ref', '-q', 'HEAD'], repository.path)
  const branch = symbolic.exitCode === 0 ? symbolic.stdout.trim() : null
  return { head, branch }
}

/**
 * The two branch tips the app has just read when it merges one branch into the
 * current one; the Rust merge refuses to run if either has moved.
 */
export async function mergeBasis(
  repository: { readonly path: string },
  sourceBranch: string
): Promise<{
  readonly sourceRef: string
  readonly sourceHead: string
  readonly currentRef: string
  readonly currentHead: string
}> {
  const { exec } = await import('dugite')
  const read = async (args: string[]) =>
    (await exec(args, repository.path)).stdout.trim()
  // Resolve by full ref name so a branch or remote named like an option
  // (`--remote/main`) is not read as one.
  let sourceRef = sourceBranch.startsWith('refs/')
    ? sourceBranch
    : `refs/heads/${sourceBranch}`
  for (const candidate of sourceBranch.startsWith('refs/')
    ? []
    : [`refs/heads/${sourceBranch}`, `refs/remotes/${sourceBranch}`]) {
    const found = await exec(
      ['show-ref', '--verify', '--quiet', '--', candidate],
      repository.path
    )
    if (found.exitCode === 0) {
      sourceRef = candidate
      break
    }
  }
  return {
    sourceRef,
    sourceHead: await read(['rev-parse', sourceRef]),
    currentRef: await read(['symbolic-ref', 'HEAD']),
    currentHead: await read(['rev-parse', 'HEAD']),
  }
}
