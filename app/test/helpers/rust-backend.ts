import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

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

export async function startRustBackend(): Promise<IRustBackend> {
  if (!rustBackendAvailable) {
    throw new Error(`Build the bridge first: ${rustBackendBinary} is missing`)
  }
  const directory = await mkdtemp(path.join(tmpdir(), 'desktop-rust-backend-'))
  // Keep the caller's Git settings out of the server's environment.
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))
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
