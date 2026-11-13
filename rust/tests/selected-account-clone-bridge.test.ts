// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { afterAll, beforeAll, describe, it, vi } from 'vitest'
import { BackendClient } from '../../app/src/backend/client'
import { BINARY, clean, object } from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

const uiRoute = vi.hoisted(() => ({ client: undefined as BackendClient | undefined }))
vi.mock('../../app/src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../app/src/backend/client')>()),
  backend: () => {
    if (uiRoute.client === undefined) throw new Error('No clone UI backend')
    return uiRoute.client
  },
}))

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
  let other: BackendClient
  let source: string
  let url: string
  let dom: { window: Window & typeof globalThis & { close(): void } }
  let uiClone: typeof import('../../app/src/ui/clone-repository/native-clone')
  let operations: typeof import('../../app/src/lib/git/repository-operation')
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
    vi.stubGlobal('location', dom.window.location)
    vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
    vi.stubGlobal('WebSocket', dom.window.WebSocket)
    client = new BackendClient(); other = new BackendClient()
    uiRoute.client = client
    uiClone = await import('../../app/src/ui/clone-repository/native-clone')
    operations = await import('../../app/src/lib/git/repository-operation')
    await bounded(Promise.all([client.waitReady(), other.waitReady()]))
  }, 360_000)
  afterAll(async () => {
    uiRoute.client = undefined
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
    assert.equal(Object.hasOwn(object(result), 'retained_staging'), false)
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

  it('runs the browser clone helper against the real bridge', async () => {
    const destination = path.join(suite.root, 'browser-clone')
    const progress: string[] = []
    const result = await bounded(uiClone.nativeClone(url, destination, account(), undefined, text => progress.push(text)), 60_000)
    assert.equal(result.path, destination)
    assert.ok(result.staging.startsWith(path.join(suite.root, '.desktop-clone-')))
    assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
    assert.ok(progress.length > 0)
    assert.deepEqual(await bounded(uiClone.listNativeCloneRecoveries()), [])
  }, 90_000)

  it('holds the renderer mutation reservation during a real transfer', async () => {
    const destination = path.join(suite.root, 'browser-reservation')
    const protection = operations.protectAssistedCommitResources(destination)
    try {
      await assert.rejects(uiClone.nativeClone(url, destination, account(), undefined, () => {}))
      assert.equal(existsSync(destination), false)
    } finally { protection.release() }
    blockTransfer()
    const cloning = uiClone.nativeClone(url, destination, account(), undefined, () => {})
    try {
      await waitForTransfer()
      assert.equal(operations.isRepositoryGitMutationInProgress(destination), true)
      unblockTransfer()
      await bounded(cloning, 60_000)
      assert.equal(operations.isRepositoryGitMutationInProgress(destination), false)
    } finally { unblockTransfer() }
  }, 90_000)

  it('returns a byte-preserving retained staging path only when requested', async () => {
    const parent = Buffer.concat([Buffer.from(`${suite.root}/parent-`), Buffer.from([0xff])])
    mkdirSync(parent)
    const destination = Buffer.concat([parent, Buffer.from('/repo')])
    const result = object(await bounded(client.request('clone_repository', {
      url, path: { bytes: Array.from(destination) }, account: account(), report_retained_staging: true,
    }), 60_000))
    assert.equal(result.kind, 'clone_completed_unselected')
    const record = object(result.record)
    const retained = object(result.retained_staging)
    const expected = Buffer.concat([parent, Buffer.from(`/.desktop-clone-${String(record.id)}`)])
    assert.deepEqual(Buffer.from(retained.bytes as number[]), expected)
    assert.ok(existsSync(Buffer.concat([expected, Buffer.from('/broker-home')])))
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
      assert.equal(Object.hasOwn(record ?? {}, 'recoverable'), false)
      assert.equal(Object.hasOwn(record ?? {}, 'conflicted'), false)
      const withStatus = await bounded(client.request('list_clone_recoveries', { report_recovery_status: true })) as Array<Record<string, unknown>>
      const status = withStatus.find(row => row.destination === destination)
      assert.equal(status?.recoverable, true)
      assert.equal(status?.conflicted, false)
      const uiRecord = (await uiClone.listNativeCloneRecoveries()).find(row => row.destination === destination)
      assert.equal(uiRecord?.recoverable, true)
      rmSync(destination, { recursive: true })
      assert.equal(typeof uiRecord?.id, 'string')
      const stage = String(uiRecord?.stagePath)
      const protection = operations.protectAssistedCommitResources(stage)
      try {
        await assert.rejects(uiClone.recoverNativeClone(String(uiRecord?.id), destination, stage))
      } finally { protection.release() }
      const recovered = await bounded(uiClone.recoverNativeClone(String(uiRecord?.id), destination, stage), 60_000)
      clean(recovered)
      assert.equal(recovered.path, destination)
      assert.equal(recovered.staging, path.dirname(String(record?.stage)))
      assert.ok(existsSync(path.join(recovered.staging, 'broker-home')))
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
      const registry = object(JSON.parse(readFileSync(suite.registry, 'utf8')))
      assert.equal(registry.selected, path.join(suite.root, 'bridge-clone'))
      assert.deepEqual(await bounded(client.request('list_clone_recoveries')), [])
    } finally { unblockTransfer() }
  }, 90_000)

  it('preserves the prior recovery response shape unless staging is requested', async () => {
    const destination = path.join(suite.root, 'bridge-recovery-legacy-response')
    blockTransfer()
    const cloning = client.request('clone_repository', { url, path: destination, account: account() })
    try {
      await waitForTransfer()
      mkdirSync(destination)
      writeFileSync(path.join(destination, 'user-data'), 'preserve')
      unblockTransfer()
      await assert.rejects(bounded(cloning, 60_000))
      const pending = await bounded(client.request('list_clone_recoveries')) as Array<Record<string, unknown>>
      const record = pending.find(row => row.destination === destination)
      assert.equal(record?.phase, 'ready')
      rmSync(destination, { recursive: true })
      const recovered = object(await bounded(client.request('recover_clone', { recovery_id: record?.id }), 60_000))
      assert.equal(recovered.path, destination)
      assert.equal(Object.hasOwn(recovered, 'retained_staging'), false)
    } finally { unblockTransfer() }
  }, 90_000)

  it('returns recovery staging bytes without UTF-8 conversion', async () => {
    const parent = Buffer.concat([Buffer.from(`${suite.root}/recovery-parent-`), Buffer.from([0xff])])
    mkdirSync(parent)
    const destination = Buffer.concat([parent, Buffer.from('/repo')])
    blockTransfer()
    const cloning = client.request('clone_repository', { url, path: { bytes: Array.from(destination) }, account: account() })
    try {
      await waitForTransfer()
      mkdirSync(destination)
      writeFileSync(Buffer.concat([destination, Buffer.from('/user-data')]), 'preserve')
      unblockTransfer()
      await assert.rejects(bounded(cloning, 60_000))
      const pending = await bounded(client.request('list_clone_recoveries')) as Array<Record<string, unknown>>
      const record = pending.find(row => typeof row.destination === 'object' && row.destination !== null && Buffer.from(object(row.destination).bytes as number[]).equals(destination))
      assert.equal(record?.phase, 'ready')
      const stage = Buffer.from(object(record?.stage).bytes as number[])
      rmSync(destination, { recursive: true })
      const recovered = object(await bounded(client.request('recover_clone', { recovery_id: record?.id, report_retained_staging: true }), 60_000))
      const retained = Buffer.from(object(recovered.retained_staging).bytes as number[])
      assert.deepEqual(retained, stage.subarray(0, stage.lastIndexOf('/')))
      assert.ok(existsSync(Buffer.concat([retained, Buffer.from('/broker-home')])))
    } finally { unblockTransfer() }
  }, 90_000)

  it.skipIf(!process.env.DESKTOP_TEST_GIT_LFS)('checks out real LFS objects using the selected account only', async () => {
    const repo = suite.seed('bridge-lfs-source', false)
    const content = Buffer.from('selected clone LFS content\0\n')
    const oid = createHash('sha256').update(content).digest('hex')
    repo.write('.gitattributes', '*.asset filter=lfs diff=lfs merge=lfs -text\n')
    repo.write('large.asset', `version https://git-lfs.github.com/spec/v1\noid sha256:${oid}\nsize ${content.length}\n`)
    repo.git('add', '.gitattributes', 'large.asset')
    repo.git('commit', '-qm', 'add LFS pointer')
    suite.service.seed(repo)
    writeFileSync(path.join(suite.service.objects, oid), content)
    symlinkSync(String(process.env.DESKTOP_TEST_GIT_LFS), path.join(suite.root, 'bin', 'git-lfs'))
    const install = spawnSync(suite.realGit, ['lfs', 'install', '--skip-repo'], { env: suite.env, encoding: 'utf8' })
    assert.equal(install.status, 0, install.stderr)
    suite.clearTransport()
    const destination = path.join(suite.root, 'bridge-lfs-clone')
    const result = await bounded(uiClone.nativeClone(url, destination, account(), undefined, () => {}), 60_000)
    assert.equal(result.path, destination)
    assert.deepEqual(readFileSync(path.join(destination, 'large.asset')), content)
    assert.ok(suite.service.records().some(row => row.path === '/fork/repo.git/info/lfs/objects/batch' && row.actor === 'alicecase'))
    suite.assertActor()
    clean(result)

    repo.write('.lfsconfig', `[lfs]\n\turl = https://${suite.service.host}/other/repo.git/info/lfs\n`)
    repo.git('add', '.lfsconfig')
    repo.git('commit', '-qm', 'configure unrelated LFS repository')
    suite.service.seed(repo)
    suite.clearTransport()
    const refused = path.join(suite.root, 'bridge-lfs-foreign')
    await assert.rejects(bounded(uiClone.nativeClone(url, refused, account(), undefined, () => {}), 60_000), error => { clean(String(error)); return true })
    assert.equal(existsSync(refused), false)
    assert.ok(!suite.service.records().some(row => String(row.path).startsWith('/other/repo.git') && row.actor !== null))
  }, 90_000)
})
