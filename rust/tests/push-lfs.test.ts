// @vitest-environment node
import { createHash } from 'node:crypto'
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import {
  mkdtemp,
  mkdir,
  readFile,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { JSDOM } from 'jsdom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../app/src/backend/client'

const lfsBinary = process.env.DESKTOP_TEST_GIT_LFS
let directory: string | undefined
let server: ChildProcess | undefined
let dom: JSDOM | undefined
let client: BackendClient | undefined

afterEach(async () => {
  client?.close()
  const child = server
  if (child && child.exitCode === null && child.signalCode === null) {
    await new Promise<void>(resolve => {
      const timeout = setTimeout(() => child.kill('SIGKILL'), 5_000)
      child.once('exit', () => {
        clearTimeout(timeout)
        resolve()
      })
      if (!child.kill('SIGTERM')) {
        clearTimeout(timeout)
        resolve()
      }
    })
  }
  dom?.window.close()
  vi.unstubAllGlobals()
  if (directory) await rm(directory, { recursive: true, force: true })
})

describe.skipIf(!lfsBinary || process.platform === 'win32')(
  'real Git LFS pre-push',
  () => {
    it('uploads the LFS object through a reviewed WebSocket push', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-lfs-push-'))
      const bin = path.join(directory, 'bin')
      await mkdir(bin)
      await symlink(lfsBinary!, path.join(bin, 'git-lfs'))
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        HOME: directory,
        PATH: `${bin}${path.delimiter}${process.env.PATH ?? ''}`,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(directory, 'global.gitconfig'),
        GIT_TERMINAL_PROMPT: '0',
      }
      for (const key of [
        'GIT_DIR',
        'GIT_WORK_TREE',
        'GIT_COMMON_DIR',
        'GIT_INDEX_FILE',
        'GIT_CONFIG_COUNT',
        'GIT_CONFIG_PARAMETERS',
        'GIT_TRACE2_EVENT',
        'GIT_LFS_PROGRESS',
      ])
        delete env[key]
      const repository = path.join(directory, 'repository')
      const remote = path.join(directory, 'remote.git')
      const git = (...args: string[]) =>
        execFileSync('git', args, {
          env,
          encoding: 'utf8',
          timeout: 20_000,
        }).trim()
      git('init', '-q', '-b', 'main', repository)
      git('init', '--bare', '-q', remote)
      git('-C', repository, 'config', 'user.name', 'Test')
      git('-C', repository, 'config', 'user.email', 'test@example.invalid')
      git('-C', repository, 'config', 'commit.gpgsign', 'false')
      git('-C', repository, 'lfs', 'install', '--local')
      git('-C', repository, 'lfs', 'track', '*.bin')
      const content = Buffer.from('real Git LFS pre-push integration\n')
      await writeFile(path.join(repository, 'file.bin'), content)
      git('-C', repository, 'add', '.gitattributes', 'file.bin')
      git('-C', repository, 'commit', '-qm', 'LFS')
      git('-C', repository, 'remote', 'add', 'origin', remote)
      const hook = await readFile(
        path.join(repository, '.git/hooks/pre-push'),
        'utf8'
      )
      expect(hook).toContain('git lfs pre-push')
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
        const timeout = setTimeout(
          () => reject(new Error('bridge startup timed out')),
          20_000
        )
        lines.once('line', line => {
          clearTimeout(timeout)
          resolve(line)
        })
        server!.once('error', reject)
        server!.once('exit', code =>
          reject(new Error(`bridge exited: ${code}`))
        )
      })
      expect(address.startsWith('Open ')).toBe(true)
      dom = new JSDOM('', { url: address.slice(5) })
      vi.stubGlobal('window', dom.window)
      vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
      vi.stubGlobal('WebSocket', dom.window.WebSocket)
      vi.stubGlobal('location', dom.window.location)
      client = new BackendClient()
      await client.waitReady()
      await client.selectBrowserRepository(repository)
      const hookEvents: string[] = []
      const output: string[] = []
      const result = await client.pushRepositoryRemote(
        repository,
        {
          remote: 'origin',
          expected_url: remote,
          local_branch: 'main',
          remote_branch: null,
          tags: [],
          force_with_lease: false,
          no_verify: false,
          report_progress: true,
          require_hook_callbacks: true,
        },
        chunk => output.push(chunk.stream),
        undefined,
        event => hookEvents.push(event.status)
      )
      expect(result).toBeUndefined()
      expect(hookEvents).toEqual(['started', 'finished'])
      expect(output.length).toBeGreaterThan(0)
      expect(git('-C', remote, 'rev-parse', 'refs/heads/main')).toBe(
        git('-C', repository, 'rev-parse', 'HEAD')
      )
      const oid = createHash('sha256').update(content).digest('hex')
      expect(
        await readFile(
          path.join(
            remote,
            'lfs/objects',
            oid.slice(0, 2),
            oid.slice(2, 4),
            oid
          )
        )
      ).toEqual(content)
    }, 90_000)
  }
)
