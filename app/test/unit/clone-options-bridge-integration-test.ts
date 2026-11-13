// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'
import { CloneProgressParser } from '../../src/lib/progress/clone'

const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM
let directory: string | undefined
let server: ChildProcess | undefined
let client: BackendClient | undefined
let dom: { window: Window & typeof globalThis & { close(): void } } | undefined

async function bounded<T>(promise: Promise<T>, ms = 15_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('operation timed out')), ms)
    })])
  } finally { if (timer !== undefined) clearTimeout(timer) }
}

afterEach(async () => {
  client?.close(); client = undefined
  dom?.window.close(); dom = undefined
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    const exited = new Promise<void>(resolve => child.once('close', () => resolve()))
    child.kill('SIGINT')
    try { await bounded(exited, 5_000) } catch { child.kill('SIGKILL'); await bounded(exited, 5_000) }
  }
  server = undefined
  vi.unstubAllGlobals()
  if (directory) await rm(directory, { recursive: true, force: true })
  directory = undefined
})

describe.skipIf(process.env.DESKTOP_TEST_BRIDGE !== '1' || process.platform === 'win32')('native clone options', () => {
  it('honors branch and recursive options while preserving old request defaults', async () => {
    directory = await mkdtemp(path.join(tmpdir(), 'desktop-clone-options-'))
    const env = { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(directory, 'missing-config'), GIT_ALLOW_PROTOCOL: 'file:http', NO_PROXY: '127.0.0.1', HTTP_PROXY: '', HTTPS_PROXY: '', ALL_PROXY: '' }
    const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { env, encoding: 'utf8' }).trim()
    const source = path.join(directory, 'source'); const sub = path.join(directory, 'sub')
    await mkdir(source); await mkdir(sub)
    for (const repo of [source, sub]) {
      git(repo, 'init', '-q', '-b', 'main')
      git(repo, 'config', 'user.name', 'Test'); git(repo, 'config', 'user.email', 'test@example.invalid'); git(repo, 'config', 'commit.gpgsign', 'false')
      await writeFile(path.join(repo, 'file'), 'initial\n')
      git(repo, 'add', 'file'); git(repo, 'commit', '-qm', 'initial')
    }
    git(source, 'submodule', 'add', '-q', sub, 'nested'); git(source, 'commit', '-qm', 'submodule')
    git(source, 'checkout', '-qb', 'feature'); await writeFile(path.join(source, 'feature'), 'selected\n')
    git(source, 'add', 'feature'); git(source, 'commit', '-qm', 'feature'); git(source, 'checkout', '-q', 'main')

    server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env, stdio: ['ignore', 'ignore', 'pipe'] })
    const lines = createInterface({ input: server.stderr! })
    const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited: ${code}`))) }))
    expect(line).toMatch(/^Open http:\/\/127\.0\.0\.1:\d+\/#token=/)
    dom = new JSDOM('', { url: line.slice(5) })
    vi.stubGlobal('window', dom!.window); vi.stubGlobal('sessionStorage', dom!.window.sessionStorage); vi.stubGlobal('WebSocket', dom!.window.WebSocket)
    client = new BackendClient(); await bounded(client.waitReady())
    const selected = path.join(directory, 'selected')
    await bounded(client.request('clone_repository', { url: source, path: selected, branch: 'feature', default_branch: 'fallback', recursive: true }), 30_000)
    expect(git(selected, 'symbolic-ref', '--short', 'HEAD')).toBe('feature')
    expect(await readFile(path.join(selected, 'nested/file'), 'utf8')).toBe('initial\n')
    const ordinary = path.join(directory, 'ordinary')
    await bounded(client.request('clone_repository', { url: source, path: ordinary }), 30_000)
    await expect(readFile(path.join(ordinary, 'nested/file'))).rejects.toMatchObject({ code: 'ENOENT' })
    const progress: string[] = []
    await bounded(client.request('clone_repository', { url: pathToFileURL(source).href, path: path.join(directory, 'progress'), report_progress: true }, {
      onProgress: chunk => progress.push(Buffer.from(chunk.data, 'base64').toString('utf8')),
    }), 30_000)
    expect(progress.length).toBeGreaterThan(0)
    expect(progress.every(line => /^(remote: )?(Enumerating objects|Counting objects|Compressing objects|Receiving objects|Resolving deltas|Checking out files|Updating files|Filtering content): (\d{1,3})% \(\3\/100\)\r$/.test(line))).toBe(true)
    const parser = new CloneProgressParser()
    expect(progress.some(line => parser.parse(line.trimEnd()).kind === 'progress')).toBe(true)

    let received!: () => void
    const receivedRequest = new Promise<void>(resolve => { received = resolve })
    let closed!: () => void
    const connectionClosed = new Promise<void>(resolve => { closed = resolve })
    const remote = createServer((request, response) => {
      request.socket.once('close', closed)
      response.writeHead(200, { 'content-type': 'application/x-git-upload-pack-advertisement' })
      response.write('001e# service=git-upload-pack\n')
      received()
    })
    await new Promise<void>(resolve => remote.listen(0, '127.0.0.1', resolve))
    try {
      const address = remote.address()
      if (!address || typeof address === 'string') throw new Error('missing test server address')
      const controller = new AbortController()
      const cloning = client.request('clone_repository', { url: `http://127.0.0.1:${address.port}/repo`, path: path.join(directory, 'cancelled'), report_progress: true }, { signal: controller.signal })
      await bounded(receivedRequest)
      controller.abort()
      await expect(cloning).rejects.toThrow()
      await bounded(connectionClosed, 10_000)
    } finally {
      remote.closeAllConnections()
      await new Promise<void>(resolve => remote.close(() => resolve()))
    }
  }, 90_000)
})
