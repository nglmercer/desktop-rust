// @vitest-environment node
import assert from 'node:assert/strict'
import { execFile, spawn, type ChildProcess } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { promisify } from 'node:util'
import { afterAll, beforeAll, describe, it, vi } from 'vitest'
import { BackendClient } from '../../app/src/backend/client'
import { BINARY } from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM
async function bounded<T>(promise: Promise<T>, ms = 30_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('operation timed out')), ms)
    })])
  } finally { if (timer !== undefined) clearTimeout(timer) }
}

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1' || process.platform !== 'linux')('selected-account clone bridge', () => {
  let suite: Credentials
  let server: ChildProcess
  let client: BackendClient
  let dom: { window: Window & typeof globalThis & { close(): void } }
  beforeAll(async () => {
    suite = new Credentials()
    await suite.start()
    await promisify(execFile)('cargo', ['build', '-p', 'desktop-cli', '--locked'], { cwd: 'rust', timeout: 300_000 })
    server = spawn(BINARY, ['--config', suite.registry, 'serve', '--empty', '--port', '0'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    const lines = createInterface({ input: server.stderr! })
    const line = await bounded(new Promise<string>((resolve, reject) => {
      lines.once('line', resolve)
      server.once('error', reject)
      server.once('exit', code => reject(new Error(`bridge exited: ${code}`)))
    }))
    assert.match(line, /^Open http:\/\/127\.0\.0\.1:\d+\/#token=/)
    dom = new JSDOM('', { url: line.slice(5) })
    vi.stubGlobal('window', dom.window)
    vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
    vi.stubGlobal('WebSocket', dom.window.WebSocket)
    client = new BackendClient()
    await bounded(client.waitReady())
  }, 360_000)
  afterAll(async () => {
    client?.close(); dom?.window.close(); vi.unstubAllGlobals()
    if (server && server.exitCode === null) {
      const exited = new Promise<void>(resolve => server.once('close', () => resolve()))
      server.kill('SIGINT')
      try { await bounded(exited, 5_000) } catch { server.kill('SIGKILL'); await bounded(exited, 5_000) }
    }
    if (suite) await suite.close()
  })

  it('rejects selected-account clones before creating a destination or contacting Git', async () => {
    const destination = path.join(suite.root, 'blocked-clone')
    suite.clearTransport()
    await assert.rejects(bounded(client.request('clone_repository', {
      url: `https://${suite.service.host}/fork/repo.git`, path: destination,
      account: { host: suite.service.host, login: 'AliceCase', id: 101 },
    })), /Selected-account cloning is temporarily unavailable/)
    assert.equal(existsSync(destination), false)
    assert.deepEqual(await bounded(client.request('list_clone_recoveries')), [])
    assert.deepEqual(suite.service.records(), [])
  })

  it('rejects recovery without changing files or creating a destination', async () => {
    const marker = path.join(suite.root, 'recovery-marker')
    writeFileSync(marker, 'preserve')
    await assert.rejects(bounded(client.request('recover_clone', {
      recovery_id: '11111111-1111-4111-8111-111111111111',
    })), /Selected-account clone recovery is temporarily unavailable/)
    assert.equal(readFileSync(marker, 'utf8'), 'preserve')
  })
})
