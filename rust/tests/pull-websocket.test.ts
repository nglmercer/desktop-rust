// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { JSDOM } from 'jsdom'
import { afterEach, expect, it, vi } from 'vitest'
import {
  BackendClient,
  FetchPreviewUnsupported,
} from '../../app/src/backend/client'

let directory: string | undefined
let server: ChildProcess | undefined
let dom: JSDOM | undefined
let client: BackendClient | undefined

afterEach(async () => {
  client?.close()
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    await new Promise<void>(resolve => {
      const timer = setTimeout(() => child.kill('SIGKILL'), 5000)
      child.once('exit', () => {
        clearTimeout(timer)
        resolve()
      })
      child.kill('SIGTERM')
    })
  }
  dom?.window.close()
  vi.unstubAllGlobals()
  if (directory) await rm(directory, { recursive: true, force: true })
})

it('pulls through a real WebSocket bridge and preserves conflicts and hook policy', async () => {
  directory = await mkdtemp(path.join(tmpdir(), 'desktop-pull-websocket-'))
  const env: NodeJS.ProcessEnv = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) =>
        !key.startsWith('GIT_') && key !== 'GH_TOKEN' && key !== 'GITHUB_TOKEN'
    )
  )
  Object.assign(env, {
    HOME: directory,
    USERPROFILE: directory,
    XDG_CONFIG_HOME: path.join(directory, 'config'),
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
    GIT_TEMPLATE_DIR: path.join(directory, 'templates'),
    GIT_TERMINAL_PROMPT: '0',
  })
  await mkdir(env.GIT_TEMPLATE_DIR!, { recursive: true })
  const git = (...args: string[]) =>
    execFileSync('git', args, { env, encoding: 'utf8', timeout: 20_000 }).trim()
  const source = path.join(directory, 'source')
  const target = path.join(directory, 'target')
  await mkdir(source)
  git('init', '-q', '-b', 'main', source)
  git('-C', source, 'config', 'user.name', 'Test')
  git('-C', source, 'config', 'user.email', 'test@example.invalid')
  git('-C', source, 'config', 'commit.gpgsign', 'false')
  await writeFile(path.join(source, 'base'), 'base\n')
  git('-C', source, 'add', 'base')
  git('-C', source, 'commit', '-qm', 'base')
  git('clone', '-q', '--no-local', source, target)
  git('-C', target, 'config', 'user.name', 'Test')
  git('-C', target, 'config', 'user.email', 'test@example.invalid')
  git('-C', target, 'config', 'commit.gpgsign', 'false')
  await writeFile(path.join(source, 'new'), 'new\n')
  git('-C', source, 'add', 'new')
  git('-C', source, 'commit', '-qm', 'new')
  const before = git('-C', target, 'rev-parse', 'HEAD')
  server = spawn(
    path.resolve('rust/target/debug/desktop-rs'),
    [
      '--config',
      path.join(directory, 'registry.json'),
      'serve',
      '--empty',
      '--port',
      '0',
    ],
    { env, stdio: ['ignore', 'ignore', 'pipe'] }
  )
  const lines = createInterface({ input: server.stderr! })
  const address = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error('bridge startup timed out')),
      20_000
    )
    lines.once('line', line => {
      clearTimeout(timer)
      resolve(line)
    })
    server!.once('error', reject)
    server!.once('exit', code => reject(new Error(`bridge exited: ${code}`)))
  })
  expect(address.startsWith('Open ')).toBe(true)
  dom = new JSDOM('', { url: address.slice(5) })
  vi.stubGlobal('window', dom.window)
  vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
  vi.stubGlobal('WebSocket', dom.window.WebSocket)
  vi.stubGlobal('location', dom.window.location)
  client = new BackendClient()
  await client.waitReady()
  await client.selectBrowserRepository(target)
  const request = {
    remote: 'origin',
    expected_url: source,
    no_verify: false,
    report_progress: false,
    require_hook_callbacks: true,
  }
  await expect(
    client.pullRepositoryRemote(target, request)
  ).rejects.toBeInstanceOf(FetchPreviewUnsupported)
  expect(git('-C', target, 'rev-parse', 'HEAD')).toBe(before)
  await mkdir(path.join(target, '.git/hooks'), { recursive: true })
  const hook = path.join(target, '.git/hooks/post-merge')
  const marker = path.join(directory, 'hook-ran')
  await writeFile(hook, `#!/bin/sh\nprintf ran > '${marker}'\n`)
  await chmod(hook, 0o755)
  await expect(
    client.pullRepositoryRemote(target, {
      ...request,
      require_hook_callbacks: false,
    })
  ).resolves.toBeUndefined()
  expect(git('-C', target, 'rev-parse', 'HEAD')).toBe(
    git('-C', source, 'rev-parse', 'HEAD')
  )
  expect(await readFile(marker, 'utf8')).toBe('ran')
  await writeFile(path.join(source, 'base'), 'remote\n')
  git('-C', source, 'add', 'base')
  git('-C', source, 'commit', '-qm', 'remote')
  await writeFile(path.join(target, 'base'), 'local\n')
  git('-C', target, 'add', 'base')
  git('-C', target, 'commit', '-qm', 'local')
  git('-C', target, 'config', 'pull.rebase', 'false')
  const failure = await client.pullRepositoryRemote(target, {
    ...request,
    require_hook_callbacks: false,
  })
  expect(failure?.errorKind).toBe('merge_conflicts')
  expect(git('-C', target, 'diff', '--name-only', '--diff-filter=U')).toBe(
    'base'
  )
}, 120_000)
