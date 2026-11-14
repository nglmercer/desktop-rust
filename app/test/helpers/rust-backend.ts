import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { afterAll, beforeAll, vi } from 'vitest'
import {
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
 * The server serves one selected repository, as the app does when the user
 * picks one. Tests build many repositories and ask about each in turn, so
 * select the repository a request is about just before binding to it.
 */
function followRequestedRepository(client: BackendClient) {
  const request = client.request.bind(client)
  let selected: string | null = null
  client.request = async (method: string, params: unknown, opts?: any) => {
    const target = (params as { path?: unknown } | null)?.path
    if (
      method === 'repository_binding' &&
      typeof target === 'string' &&
      target !== selected
    ) {
      await client.selectBrowserRepository(target)
      selected = target
    }
    return request(method, params, opts)
  }
}

export async function startRustBackend(): Promise<IRustBackend> {
  if (!rustBackendAvailable) {
    throw new Error(`Build the bridge first: ${rustBackendBinary} is missing`)
  }
  const directory = await mkdtemp(path.join(tmpdir(), 'desktop-rust-backend-'))
  // Keep the caller's Git settings out of the server's environment, except the
  // author and committer the test environment defines for its commits.
  const env = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) =>
        !key.startsWith('GIT_') ||
        /^GIT_(AUTHOR|COMMITTER)_(NAME|EMAIL)$/.test(key)
    )
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
      env: { ...env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' },
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
    await rm(directory, { recursive: true, force: true })
    throw error
  }

  const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
    JSDOM: new (html: string, options: { url: string }) => IDom
  }
  const dom = new JSDOM('', { url })
  vi.stubGlobal('window', dom.window)
  vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
  vi.stubGlobal('WebSocket', dom.window.WebSocket)
  vi.stubGlobal('location', dom.window.location)
  const client = new BackendClient()
  await bounded(client.waitReady())
  followRequestedRepository(client)

  return {
    client,
    directory,
    async stop() {
      client.close()
      dom.window.close()
      vi.unstubAllGlobals()
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
  const sourceRef =
    (await read(['rev-parse', '--symbolic-full-name', sourceBranch])) ||
    `refs/heads/${sourceBranch}`
  return {
    sourceRef,
    sourceHead: await read(['rev-parse', sourceRef]),
    currentRef: await read(['symbolic-ref', 'HEAD']),
    currentHead: await read(['rev-parse', 'HEAD']),
  }
}
