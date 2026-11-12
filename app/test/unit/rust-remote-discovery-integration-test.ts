// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it, vi } from 'vitest'

const legacyGit = vi.hoisted(() => vi.fn())
vi.mock('../../src/lib/git/core', () => ({ git: legacyGit }))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: vi.fn(),
}))

import { backend } from '../../src/backend/client'
import {
  addRemote,
  getRemoteHEAD,
  getRemoteURL,
  getRemotes,
  removeRemote,
  setRemoteURL,
} from '../../src/lib/git/remote'
import { getSymbolicRef } from '../../src/lib/git/refs'
import type { Repository } from '../../src/models/repository'

const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM
let directory: string | undefined
let server: ChildProcess | undefined
let dom: { window: Window & typeof globalThis & { close(): void } } | undefined
let client: ReturnType<typeof backend> | undefined

afterEach(async () => {
  client?.close()
  dom?.window.close()
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    const exited = new Promise<void>(resolve =>
      child.once('exit', () => resolve())
    )
    child.kill('SIGINT')
    await exited
  }
  vi.unstubAllGlobals()
  if (directory) await rm(directory, { recursive: true, force: true })
})

describe.skipIf(
  process.env.DESKTOP_TEST_BRIDGE !== '1' || process.platform === 'win32'
)('real remote discovery', () => {
  it('resolves nested and bare repositories and handles non-repositories through Rust', async () => {
    directory = await mkdtemp(path.join(tmpdir(), 'desktop-remote-discovery-'))
    const repository = path.join(directory, 'repository')
    const nested = path.join(repository, 'nested')
    execFileSync('git', ['init', '-q', repository])
    await mkdir(nested)
    execFileSync('git', [
      '-C',
      repository,
      'config',
      'url.https://example.invalid/.insteadOf',
      'work:',
    ])
    execFileSync('git', [
      '-C',
      repository,
      'config',
      'remote.origin.url',
      'work:owner/repo',
    ])
    const child = spawn(
      path.resolve('rust/target/debug/desktop-rs'),
      [
        '--config',
        path.join(directory, 'registry.json'),
        'serve',
        '--empty',
        '--port',
        '0',
      ],
      {
        env: {
          ...process.env,
          HOME: directory,
          GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
          GIT_CONFIG_NOSYSTEM: '1',
        },
        stdio: ['ignore', 'ignore', 'pipe'],
      }
    )
    server = child
    if (!child.stderr) throw new Error('Bridge stderr is unavailable')
    const lines = createInterface({ input: child.stderr })
    const announcement = await new Promise<string>((resolve, reject) => {
      lines.once('line', resolve)
      child.once('error', reject)
      child.once('exit', code =>
        reject(new Error(`bridge exited before startup: ${code}`))
      )
    })
    expect(announcement.startsWith('Open ')).toBe(true)
    const browser = new JSDOM('', {
      url: announcement.slice(5),
    }) as NonNullable<typeof dom>
    dom = browser
    vi.stubGlobal('window', browser.window)
    vi.stubGlobal('sessionStorage', browser.window.sessionStorage)
    vi.stubGlobal('WebSocket', browser.window.WebSocket)
    client = backend()
    await client.waitReady()
    expect(await client.snapshot()).toBeNull()
    await expect(getRemotes({ path: nested } as Repository)).resolves.toEqual([
      { name: 'origin', url: 'https://example.invalid/owner/repo' },
    ])
    await expect(
      getRemoteURL({ path: nested } as Repository, 'origin')
    ).resolves.toBe('https://example.invalid/owner/repo')
    await expect(
      getRemoteURL({ path: nested } as Repository, 'missing')
    ).resolves.toBeNull()
    await expect(
      getRemoteURL({ path: nested } as Repository, '--upload-pack=invalid')
    ).resolves.toBeNull()
    execFileSync('git', [
      '-C',
      repository,
      'config',
      'remote.private.url',
      'https://user:synthetic-secret@example.invalid/repo',
    ])
    await expect(
      client.repositorySafeRemoteUrl(repository, 'private')
    ).resolves.toEqual({ kind: 'unsupported' })
    const plain = path.join(directory, 'plain')
    await mkdir(plain)
    await writeFile(
      path.join(directory, 'gitconfig'),
      '[remote "global"]\n\turl = https://example.invalid/global\n'
    )
    await expect(getRemotes({ path: plain } as Repository)).resolves.toEqual([])
    await expect(
      getRemoteURL({ path: plain } as Repository, 'global')
    ).resolves.toBeNull()
    await rm(path.join(directory, 'gitconfig'))
    const bare = path.join(directory, 'bare.git')
    execFileSync('git', ['init', '--bare', '-q', bare])
    execFileSync('git', [
      '-C',
      bare,
      'config',
      'remote.origin.url',
      'https://example.invalid/bare',
    ])
    await expect(getRemotes({ path: bare } as Repository)).resolves.toEqual([
      { name: 'origin', url: 'https://example.invalid/bare' },
    ])
    await expect(
      getRemoteURL({ path: bare } as Repository, 'origin')
    ).resolves.toBe('https://example.invalid/bare')
    await expect(
      getRemotes({ path: path.join(directory, 'missing') } as Repository)
    ).rejects.toThrow()
    const broken = path.join(directory, 'broken')
    execFileSync('git', ['init', '-q', broken])
    await writeFile(path.join(broken, '.git', 'config'), '[broken\n')
    await expect(getRemotes({ path: broken } as Repository)).rejects.toThrow()
    await expect(
      getRemoteURL({ path: broken } as Repository, 'origin')
    ).rejects.toThrow()
    await expect(
      removeRemote({ path: broken } as Repository, 'origin')
    ).rejects.toThrow()
    const symbolicRepository = { path: nested } as Repository
    const initialHead = execFileSync(
      'git',
      ['-C', repository, 'symbolic-ref', '-q', 'HEAD'],
      { encoding: 'utf8' }
    ).trim()
    await expect(getSymbolicRef(symbolicRepository, 'HEAD')).resolves.toBe(
      initialHead
    )
    await expect(
      getSymbolicRef(symbolicRepository, 'refs/remotes/origin/HEAD')
    ).resolves.toBeNull()
    execFileSync('git', [
      '-C',
      repository,
      'symbolic-ref',
      'refs/remotes/origin/HEAD',
      'refs/remotes/origin/main',
    ])
    await expect(getRemoteHEAD(symbolicRepository, 'origin')).resolves.toBe(
      'main'
    )
    await expect(
      getSymbolicRef(symbolicRepository, '--invalid')
    ).rejects.toThrow()
    execFileSync('git', [
      '-C',
      repository,
      '-c',
      'user.name=Test',
      '-c',
      'user.email=test@example.invalid',
      '-c',
      'commit.gpgsign=false',
      '-c',
      'core.hooksPath=/dev/null',
      'commit',
      '--allow-empty',
      '-qm',
      'test',
    ])
    execFileSync('git', ['-C', repository, 'checkout', '--detach', '--quiet'])
    await expect(getSymbolicRef(symbolicRepository, 'HEAD')).resolves.toBeNull()
    const mutations = path.join(directory, 'mutations')
    execFileSync('git', ['init', '-q', mutations])
    const mutationRepository = { path: mutations } as Repository
    await expect(
      addRemote(mutationRepository, 'origin', 'https://example.invalid/one')
    ).resolves.toEqual({ name: 'origin', url: 'https://example.invalid/one' })
    expect(
      execFileSync(
        'git',
        ['-C', mutations, 'config', '--local', '--get', 'remote.origin.url'],
        { encoding: 'utf8' }
      )
    ).toBe('https://example.invalid/one\n')
    await expect(
      addRemote(mutationRepository, 'origin', 'https://example.invalid/other')
    ).rejects.toThrow()
    await expect(
      setRemoteURL(mutationRepository, 'origin', 'https://example.invalid/two')
    ).resolves.toBe(true)
    expect(
      execFileSync(
        'git',
        ['-C', mutations, 'config', '--local', '--get', 'remote.origin.url'],
        { encoding: 'utf8' }
      )
    ).toBe('https://example.invalid/two\n')
    await expect(
      client.manageRepositoryRemote(mutations, {
        type: 'set_url',
        name: 'origin',
        url: 'https://user:synthetic-secret@example.invalid/repo',
      })
    ).resolves.toBe('unsupported')
    expect(
      execFileSync(
        'git',
        ['-C', mutations, 'config', '--local', '--get', 'remote.origin.url'],
        { encoding: 'utf8' }
      )
    ).toBe('https://example.invalid/two\n')
    await removeRemote(mutationRepository, 'origin')
    await removeRemote(mutationRepository, 'origin')
    expect(
      execFileSync('git', ['-C', mutations, 'remote'], { encoding: 'utf8' })
    ).toBe('')
    expect(legacyGit).not.toHaveBeenCalled()
    expect(await client.snapshot()).toBeNull()
  }, 30_000)
})
