// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { afterAll, beforeAll, describe, it, vi } from 'vitest'
import { BackendClient } from '../../app/src/backend/client'
import { BINARY, clean, object } from './fixtures/accounts'
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

describe.skip('selected-account clone bridge (disabled pending safe child-local descriptor lifetime)', () => {
  let suite: Credentials
  let server: ChildProcess
  let client: BackendClient
  let other: BackendClient
  let source: string
  let url: string
  let dom: { window: Window & typeof globalThis & { close(): void } }
  beforeAll(async () => {
    suite = new Credentials()
    await suite.start()
    source = suite.seed('bridge-source', false).root
    url = `https://${suite.service.host}/fork/repo.git`
    const build = spawnSync('cargo', ['build', '-p', 'desktop-cli', '--locked'], { cwd: 'rust', encoding: 'utf8', timeout: 300_000 })
    assert.equal(build.status, 0, build.stderr)
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
    client = new BackendClient(); other = new BackendClient()
    await bounded(Promise.all([client.waitReady(), other.waitReady()]))
  }, 360_000)
  afterAll(async () => {
    client?.close(); other?.close(); dom?.window.close(); vi.unstubAllGlobals()
    if (server && server.exitCode === null) {
      const exited = new Promise<void>(resolve => server.once('close', () => resolve()))
      server.kill('SIGINT')
      try { await bounded(exited, 5_000) } catch { server.kill('SIGKILL'); await bounded(exited, 5_000) }
    }
    if (suite) await suite.close()
  })
  const account = () => ({ host: suite.service.host, login: 'AliceCase', id: 101 })
  async function waitForTransfer() {
    const deadline = Date.now() + 30_000
    while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
    assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
  }
  function blockTransfer() {
    rmSync(suite.service.started, { force: true }); rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
  }
  function unblockTransfer() {
    writeFileSync(suite.service.release, 'release')
    suite.service.update({ blockPath: undefined })
  }

  it('clones with its verified account and leaves selection explicit', async () => {
    const destination = path.join(suite.root, 'bridge-clone')
    suite.clearTransport()
    const progress: string[] = []
    const result = await bounded(client.request('clone_repository', { url, path: destination, account: account(), report_progress: true }, { onProgress: chunk => progress.push(Buffer.from(chunk.data, 'base64').toString('utf8')) }), 60_000)
    clean(result); clean(progress)
    assert.equal(object(result).kind, 'clone_completed_unselected')
    assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
    suite.assertActor()
    const registry = object(JSON.parse(readFileSync(suite.registry, 'utf8')))
    assert.equal(registry.selected ?? null, null)
    const record = (registry.records as Array<Record<string, unknown>>).find(row => row.path === destination)
    assert.deepEqual(object(object(record?.account).assignment), { mode: 'assigned', ...account() })
    assert.ok(progress.length > 0, 'expected clone progress notifications')
    assert.ok(progress.every(line => /^(remote: )?(Enumerating objects|Counting objects|Compressing objects|Receiving objects|Resolving deltas|Checking out files|Updating files|Filtering content): (\d{1,3})% \(\3\/100\)\r$/.test(line)))
    await bounded(client.request('open_repository', { path: destination }))
  }, 90_000)

  it('checks out the requested branch and refuses unsupported options before staging', async () => {
    const remote = path.join(suite.service.projects, 'fork', 'repo.git')
    const created = spawnSync(suite.realGit, ['--git-dir', remote, 'branch', 'feature', 'main'], { encoding: 'utf8' })
    assert.equal(created.status, 0, created.stderr)
    const destination = path.join(suite.root, 'bridge-feature')
    const result = object(await bounded(client.request('clone_repository', { url, path: destination, account: account(), branch: 'feature' }), 60_000))
    assert.equal(result.kind, 'clone_completed_unselected')
    const branch = spawnSync(suite.realGit, ['-C', destination, 'symbolic-ref', '--short', 'HEAD'], { env: suite.env, encoding: 'utf8' })
    assert.equal(branch.status, 0, branch.stderr)
    assert.equal(branch.stdout.trim(), 'feature')
    for (const [name, options] of [
      ['invalid-branch', { branch: '-invalid' }],
      ['unsupported-recursive', { recursive: true }],
    ] as const) {
      const rejected = path.join(suite.root, name)
      await assert.rejects(bounded(client.request('clone_repository', { url, path: rejected, account: account(), ...options }), 60_000))
      assert.equal(existsSync(rejected), false)
    }
    assert.deepEqual(await bounded(client.request('list_clone_recoveries')), [])
  }, 90_000)

  it('keeps a newer A-to-B-to-A selection when a clone finishes on another connection', async () => {
    const destination = path.join(suite.root, 'bridge-concurrent')
    blockTransfer()
    const cloning = client.request('clone_repository', { url, path: destination, account: account() })
    try {
      await waitForTransfer()
      await bounded(other.request('open_repository', { path: source }))
      await bounded(other.request('open_repository', { path: path.join(suite.root, 'bridge-clone') }))
      unblockTransfer()
      const result = object(await bounded(cloning, 60_000))
      assert.equal(result.kind, 'clone_completed_unselected')
      const registry = object(JSON.parse(readFileSync(suite.registry, 'utf8')))
      assert.equal(registry.selected, path.join(suite.root, 'bridge-clone'))
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
    } finally { unblockTransfer() }
  }, 90_000)

  it('lists and recovers a completed stage without changing the current selection', async () => {
    const destination = path.join(suite.root, 'bridge-recovery')
    blockTransfer()
    const cloning = client.request('clone_repository', { url, path: destination, account: account() })
    try {
      await waitForTransfer()
      mkdirSync(destination); writeFileSync(path.join(destination, 'user-data'), 'preserve')
      unblockTransfer()
      await assert.rejects(bounded(cloning, 60_000))
      assert.equal(readFileSync(path.join(destination, 'user-data'), 'utf8'), 'preserve')
      const pending = await bounded(client.request('list_clone_recoveries')) as Array<Record<string, unknown>>
      const record = pending.find(row => row.destination === destination)
      assert.equal(record?.phase, 'ready')
      rmSync(destination, { recursive: true })
      const recovered = object(await bounded(client.request('recover_clone', { recovery_id: record?.id }), 60_000))
      clean(recovered)
      assert.equal(recovered.path, destination)
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
      const registry = object(JSON.parse(readFileSync(suite.registry, 'utf8')))
      assert.equal(registry.selected, path.join(suite.root, 'bridge-clone'))
      assert.deepEqual(await bounded(client.request('list_clone_recoveries')), [])
    } finally { unblockTransfer() }
  }, 90_000)
})
