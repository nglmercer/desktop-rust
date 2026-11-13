import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { createInterface } from 'node:readline'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it, vi } from 'vitest'
import { FetchStreamProgressParser } from '../../app/src/lib/progress/fetch-stream'
import { BackendClient, type FetchProgressChunk } from '../../app/src/backend/client'

// Explicit opt-in keeps ordinary test runs portable when Git LFS is absent.
const gitLfs = process.env.DESKTOP_TEST_GIT_LFS

async function bounded<T>(operation: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('operation timed out')), ms) }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

describe.skipIf(!gitLfs || process.platform === 'win32')('real Git LFS progress', () => {
  const runFixture = async (viaRust: boolean) => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-real-lfs-'))
    const bin = path.join(root, 'bin')
    mkdirSync(bin)
    symlinkSync(gitLfs!, path.join(bin, 'git-lfs'))
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: root, PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(root, 'global'), GIT_TERMINAL_PROMPT: '0' }
    for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS', 'GIT_EXEC_PATH', 'GIT_LFS_PROGRESS']) delete env[key]
    const git = (args: string[]) => {
      const result = spawnSync('git', args, { env, encoding: 'utf8', timeout: 10_000 })
      assert.ifError(result.error)
      assert.equal(result.status, 0, result.stderr)
    }
    const content = Buffer.alloc(256 * 1024, 0x61)
    const oid = createHash('sha256').update(content).digest('hex')
    let batchRequests = 0
    let downloads = 0
    const server = createServer((request, response) => {
      if (request.method === 'POST' && request.url === '/lfs/objects/batch') {
        const chunks: Buffer[] = []
        request.on('data', chunk => chunks.push(Buffer.from(chunk)))
        request.on('end', () => {
          const body = JSON.parse(Buffer.concat(chunks).toString())
          if (body.operation !== 'download' || body.objects?.[0]?.oid !== oid) {
            response.writeHead(400).end()
            return
          }
          batchRequests++
          const address = server.address()
          assert.ok(address && typeof address !== 'string')
          response.writeHead(200, { 'content-type': 'application/vnd.git-lfs+json' })
          response.end(JSON.stringify({ transfer: 'basic', objects: [{ oid, size: content.length, actions: { download: { href: `http://127.0.0.1:${address.port}/objects/${oid}` } } }] }))
        })
      } else if (request.method === 'GET' && request.url === `/objects/${oid}`) {
        downloads++
        response.writeHead(200, { 'content-length': content.length })
        response.end(content)
      } else {
        response.writeHead(404).end()
      }
    })
    try {
      await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
      const address = server.address()
      assert.ok(address && typeof address !== 'string')
      const source = path.join(root, 'source')
      const remote = path.join(root, 'remote.git')
      const target = path.join(root, 'target')
      git(['init', '-q', '-b', 'main', source])
      git(['-C', source, 'config', 'user.name', 'Test'])
      git(['-C', source, 'config', 'user.email', 'test@example.invalid'])
      writeFileSync(path.join(source, '.gitattributes'), '*.bin filter=lfs diff=lfs merge=lfs -text\n')
      writeFileSync(path.join(source, 'file.bin'), `version https://git-lfs.github.com/spec/v1\noid sha256:${oid}\nsize ${content.length}\n`)
      git(['-C', source, 'add', '.gitattributes', 'file.bin'])
      git(['-C', source, '-c', 'commit.gpgsign=false', 'commit', '-qm', 'pointer'])
      git(['init', '--bare', '-q', remote])
      git(['-C', source, 'push', '-q', remote, 'HEAD:refs/heads/main'])
      git(['init', '-q', target])
      git(['-C', target, 'remote', 'add', 'origin', remote])
      git(['-C', target, 'fetch', '-q', 'origin', 'main'])
      git(['-C', target, 'config', 'remote.origin.lfsurl', `http://127.0.0.1:${address.port}/lfs`])
      const progressFile = path.join(root, 'progress')
      writeFileSync(progressFile, '')
      if (viaRust) {
        // A normal `git fetch` does not itself download LFS objects. This
        // configured SSH transport deliberately invokes real Git LFS before
        // serving upload-pack, exercising inherited runner progress capture.
        const ssh = path.join(root, 'ssh-wrapper')
        writeFileSync(ssh, '#!/bin/sh\ngit -C "$DESKTOP_LFS_FIXTURE_TARGET" lfs fetch origin refs/remotes/origin/main >&2 || exit $?\nexec git-upload-pack "$DESKTOP_LFS_FIXTURE_SOURCE"\n')
        chmodSync(ssh, 0o700)
        // Git evaluates core.sshCommand as shell syntax, so quote its path
        // even though all arguments passed to `git` above are already separate.
        git(['-C', target, 'config', 'core.sshCommand', `'${ssh.replace(/'/g, "'\\''")}'`])
        git(['-C', target, 'config', 'ssh.variant', 'simple'])
        const remoteUrl = `git@fixture.invalid:${source}`
        git(['-C', target, 'remote', 'add', 'stream', remoteUrl])
        const desktop = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(root, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...env, DESKTOP_LFS_FIXTURE_TARGET: target, DESKTOP_LFS_FIXTURE_SOURCE: source }, stdio: ['ignore', 'ignore', 'pipe'], detached: true })
        const readLine = createInterface({ input: desktop.stderr! })
        let client: BackendClient | undefined
        let dom: { window: Window & typeof globalThis & { close(): void } } | undefined
        try {
          const line = await bounded(new Promise<string>((resolve, reject) => { readLine.once('line', resolve); desktop.once('error', reject); desktop.once('exit', code => reject(new Error(`bridge exited: ${code}`))) }), 10_000)
          assert.ok(line.startsWith('Open '))
          const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM
          dom = new JSDOM('', { url: line.slice(5) })
          vi.stubGlobal('window', dom!.window)
          vi.stubGlobal('sessionStorage', dom!.window.sessionStorage)
          vi.stubGlobal('WebSocket', dom!.window.WebSocket)
          client = new BackendClient()
          await bounded(client.waitReady(), 10_000)
          await bounded(client.selectBrowserRepository(target), 10_000)
          const chunks: FetchProgressChunk[] = []
          const failure = await bounded(client.fetchRepositoryRemote(target, 'stream', remoteUrl, false, false, undefined, chunk => chunks.push(chunk)), 15_000)
          assert.equal(failure, undefined)
          assert.ok(chunks.some(chunk => chunk.stream === 'lfs'), 'Rust runner should emit real Git LFS progress')
          const decoded: unknown[] = []
          const parser = new FetchStreamProgressParser(value => decoded.push(value))
          chunks.filter(chunk => chunk.stream === 'lfs').forEach(chunk => parser.push(chunk))
          parser.finish()
          assert.ok(decoded.some(value => typeof value === 'object' && value !== null && 'kind' in value && value.kind === 'progress'))
        } finally {
          client?.close()
          dom?.window.close()
          vi.unstubAllGlobals()
          readLine.close()
          if (desktop.exitCode === null && desktop.signalCode === null) {
            const exited = new Promise<void>(resolve => desktop.once('close', () => resolve()))
            desktop.kill('SIGINT')
            try { await bounded(exited, 3_000) } catch {
              if (desktop.pid !== undefined) {
                try { process.kill(-desktop.pid, 'SIGKILL') } catch { /* Already exited. */ }
              }
              await bounded(exited, 3_000)
            }
          }
        }
      } else await new Promise<void>((resolve, reject) => {
        const child = spawn('git', ['-C', target, 'lfs', 'fetch', 'origin', 'refs/remotes/origin/main'], { env: { ...env, GIT_LFS_PROGRESS: progressFile }, stdio: ['ignore', 'ignore', 'pipe'], detached: true })
        let stderr = ''
        let timedOut = false
        child.stderr.setEncoding('utf8')
        child.stderr.on('data', chunk => { stderr += chunk })
        const timer = setTimeout(() => {
          timedOut = true
          if (child.pid !== undefined) {
            try { process.kill(-child.pid, 'SIGKILL') } catch { /* The process has already exited. */ }
          }
        }, 15_000)
        child.once('error', error => { clearTimeout(timer); reject(error) })
        child.once('close', code => { clearTimeout(timer); timedOut ? reject(new Error('Git LFS transfer timed out')) : code === 0 ? resolve() : reject(new Error(`Git LFS exited ${code}: ${stderr}`)) })
      })
      assert.equal(batchRequests, 1)
      assert.equal(downloads, 1)
      assert.deepEqual(readFileSync(path.join(target, '.git', 'lfs', 'objects', oid.slice(0, 2), oid.slice(2, 4), oid)), content)
      if (!viaRust) {
        const bytes = readFileSync(progressFile)
        assert.ok(bytes.length > 0, 'real Git LFS transfer must write progress')
        const progress: unknown[] = []
        const parser = new FetchStreamProgressParser(value => progress.push(value))
        parser.push({ stream: 'lfs', data: bytes.toString('base64') })
        parser.finish()
        assert.ok(progress.some(value => typeof value === 'object' && value !== null && 'kind' in value && value.kind === 'progress'))
      }
    } finally {
      server.closeAllConnections()
      await new Promise<void>(resolve => server.close(() => resolve()))
      rmSync(root, { recursive: true, force: true })
    }
  }
  it('parses the progress file generated by an actual local HTTP LFS transfer', () => runFixture(false), 30_000)
  it.skipIf(process.env.DESKTOP_TEST_BRIDGE !== '1')('streams real LFS progress through the Rust fetch runner', () => runFixture(true), 30_000)
})
