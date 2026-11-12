// @vitest-environment node
import {
  execFileSync,
  spawn,
  spawnSync,
  type ChildProcess,
} from 'node:child_process'
import {
  mkdtemp,
  mkdir,
  readFile,
  rm,
  writeFile,
  chmod,
  unlink,
  symlink,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'
import { getReviewedCopilotLines } from '../../src/lib/copilot/commit-selection'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffSelection,
  DiffSelectionType,
  DiffType,
} from '../../src/models/diff'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'

const enabled = process.env.DESKTOP_TEST_BRIDGE === '1'
interface IDom {
  readonly window: Window & typeof globalThis & { close(): void }
}
const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM as new (
  html: string,
  options: { url: string }
) => IDom
const clients: BackendClient[] = []
let server: ChildProcess | undefined
let directory: string | undefined
let dom: IDom | undefined

async function bounded<T>(
  promise: Promise<T>,
  milliseconds = 10_000
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('integration operation timed out')),
          milliseconds
        )
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function waitFor(
  predicate: () => boolean | Promise<boolean>
): Promise<void> {
  const deadline = Date.now() + 10_000
  while (!(await predicate())) {
    if (Date.now() >= deadline)
      throw new Error('integration condition timed out')
    await new Promise(resolve => setTimeout(resolve, 20))
  }
}

afterEach(async () => {
  for (const value of clients.splice(0)) value.close()
  dom?.window.close()
  dom = undefined
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    const exited = new Promise<void>(resolve =>
      child.once('exit', () => resolve())
    )
    child.kill('SIGINT')
    try {
      await bounded(exited)
    } catch {
      child.kill('SIGKILL')
      await bounded(exited)
    }
  }
  server = undefined
  vi.unstubAllGlobals()
  if (directory) await rm(directory, { recursive: true, force: true })
  directory = undefined
})

describe.skipIf(!enabled || process.platform === 'win32')(
  'real Rust WebSocket bridge',
  () => {
    it('authenticates, cancels work, delivers events and reconnects without replay', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-websocket-test-'))
      const bin = path.join(directory, 'gh')
      const marker = path.join(directory, 'gh.pid')
      await writeFile(
        bin,
        '#!/bin/sh\nprintf "%s\\n" "$$" >> "$BRIDGE_GH_PID"\nexec sleep 60\n'
      )
      await chmod(bin, 0o700)
      await writeFile(
        path.join(directory, 'git config with spaces'),
        '[user]\n\tname = Bridge Test\n\temail = bridge@example.invalid\n[core]\n\tautocrlf = input\n'
      )
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
        {
          env: {
            ...process.env,
            HOME: directory,
            PATH: `${directory}${path.delimiter}${process.env.PATH ?? ''}`,
            BRIDGE_GH_PID: marker,
            GIT_CONFIG_NOSYSTEM: '1',
            GIT_EDITOR: 'false',
            GIT_CONFIG_GLOBAL: path.join(directory, 'git config with spaces'),
            GIT_AUTHOR_NAME: 'Environment Author',
            GIT_AUTHOR_EMAIL: 'environment@example.invalid',
            GIT_COMMITTER_NAME: undefined,
            GIT_COMMITTER_EMAIL: undefined,
          },
          stdio: ['ignore', 'ignore', 'pipe'],
        }
      )
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(
        new Promise<string>((resolve, reject) => {
          lines.once('line', resolve)
          server!.once('error', reject)
          server!.once('exit', code =>
            reject(new Error(`bridge exited before startup: ${code}`))
          )
        })
      )
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      expect(/^http:\/\/127\.0\.0\.1:\d+\/#token=/.test(sessionUrl)).toBe(true)
      dom = new JSDOM('', { url: sessionUrl })
      const sockets: WebSocket[] = []
      const NativeSocket = dom.window.WebSocket
      class TrackedSocket extends NativeSocket {
        constructor(url: string | URL, protocols?: string | string[]) {
          super(url, protocols)
          sockets.push(this as unknown as WebSocket)
        }
      }
      vi.stubGlobal('window', dom.window)
      vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
      vi.stubGlobal('WebSocket', TrackedSocket)
      const client = new BackendClient()
      clients.push(client)
      expect((await bounded(client.waitReady())).version).toBe(3)
      const configPath = path.join(directory, 'git config with spaces')
      const savedConfig = await readFile(configPath)
      await unlink(configPath)
      expect(await client.globalGitConfigPath()).toBe(configPath)
      await readFile(configPath)
      await writeFile(configPath, savedConfig)
      expect(await bounded(client.gitVersion())).toBe(execFileSync('git', ['--version'], { encoding: 'utf8' }).trim().replace(/^git version /, ''))
      expect(await bounded(client.request('git_settings', { scope: 'global' }))).toMatchObject({
        scope: 'global',
        values: { 'user.name': 'Bridge Test', 'user.email': 'bridge@example.invalid' },
      })
      expect(await client.request('set_git_setting', { scope: 'global', key: 'init.defaultbranch', value: 'trunk' })).toMatchObject({ values: { 'init.defaultbranch': 'trunk' } })
      await expect(client.request('set_git_setting', { scope: 'global', key: 'init.defaultbranch', value: 'bad..branch' })).rejects.toThrow()
      expect(await client.request('set_git_setting', { scope: 'global', key: 'init.defaultbranch', value: null })).toMatchObject({ values: { 'init.defaultbranch': null } })
      expect(await bounded(client.request('set_git_setting', { scope: 'global', key: 'user.name', value: '' }))).toMatchObject({ values: { 'user.name': '', 'user.email': 'bridge@example.invalid' } })
      expect(await bounded(client.request('git_settings', { scope: 'global' }))).toMatchObject({ values: { 'user.name': '' } })
      await expect(bounded(client.request('set_git_setting', { scope: 'global', key: 'credential.helper', value: 'unsafe' }))).rejects.toThrow()
      expect(await bounded(client.request('set_git_setting', { scope: 'global', key: 'user.name', value: 'Bridge Test' }))).toMatchObject({ values: { 'user.name': 'Bridge Test' } })
      expect(dom.window.location.hash).toBe('')
      await expect(bounded(client.snapshot())).resolves.toBeNull()
      const beforeSelection = path.join(directory, 'before selection')
      await client.initializeDesktopRepository(beforeSelection, 'main')
      expect(await readFile(path.join(beforeSelection, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/main\n')
      await expect(client.snapshot()).resolves.toBeNull()
      await expect(client.directoryExists(directory)).resolves.toBe(true)
      await expect(client.pathExists(directory)).resolves.toBe(true)
      await expect(
        client.directoryExists(path.join(directory, 'missing'))
      ).resolves.toBe(false)
      await expect(
        client.pathExists(path.join(directory, 'missing'))
      ).resolves.toBe(false)
      const dangling = path.join(directory, 'dangling')
      await symlink(path.join(directory, 'missing'), dangling)
      await expect(client.pathExists(dangling)).resolves.toBe(false)
      const bare = path.join(directory, 'bare.git')
      execFileSync('git', ['init', '--bare', '-q', bare])
      execFileSync('git', ['-C', bare, 'config', 'remote.origin.url', 'https://example.invalid/bare'])
      await expect(client.repositorySafeFetchRemotes(bare)).resolves.toEqual([{ name: 'origin', url: 'https://example.invalid/bare' }])
      await expect(client.repositorySafeFetchRemotes(directory)).resolves.toEqual([])
      await expect(client.repositoryType(bare)).resolves.toEqual({
        kind: 'bare',
      })
      await expect(
        client.repositoryType(path.join(directory, 'missing'))
      ).resolves.toEqual({ kind: 'missing' })

      const firstRepository = path.join(directory, 'first')
      const secondRepository = path.join(directory, 'second')
      await bounded(
        client.request('init_repository', { path: firstRepository })
      )
      const initialSelection = await client.snapshot()
      const existingFolder = path.join(directory, 'existing folder')
      await mkdir(existingFolder)
      await writeFile(path.join(existingFolder, 'keep.txt'), 'existing content')
      await client.initializeDesktopRepository(existingFolder, 'trunk')
      expect(await readFile(path.join(existingFolder, 'keep.txt'), 'utf8')).toBe('existing content')
      expect(await readFile(path.join(existingFolder, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/trunk\n')
      expect(await client.repositorySafeFetchRemotes(existingFolder)).toEqual([])
      execFileSync('git', ['-C', existingFolder, 'config', 'url.https://example.invalid/.insteadOf', 'work:'])
      execFileSync('git', ['-C', existingFolder, 'config', '--add', 'remote.origin.url', 'work:owner/repo'])
      expect(await client.repositorySafeFetchRemotes(existingFolder)).toEqual([{ name: 'origin', url: 'https://example.invalid/owner/repo' }])
      execFileSync('git', ['-C', existingFolder, 'config', '--unset-all', 'remote.origin.url'])
      execFileSync('git', ['-C', existingFolder, 'config', '--add', 'remote.origin.url', 'https://example.invalid/repo?token=synthetic-secret'])
      expect(await client.repositorySafeFetchRemotes(existingFolder)).toBeNull()
      execFileSync('git', ['-C', existingFolder, 'config', '--unset-all', 'remote.origin.url'])
      execFileSync('git', ['-C', existingFolder, 'config', '--add', 'remote.origin.url', 'https://user:synthetic-secret@example.invalid/repo'])
      expect(await client.repositorySafeFetchRemotes(existingFolder)).toBeNull()
      execFileSync('git', ['-C', existingFolder, 'config', '--add', 'remote.origin.url', ''])
      expect(await client.repositorySafeFetchRemotes(existingFolder)).toEqual([])
      execFileSync('git', ['-C', existingFolder, 'config', '--unset-all', 'remote.origin.url'])
      await client.writeRepositoryCreationFile(existingFolder, 'readme', '# New 🦀\n')
      expect(await readFile(path.join(existingFolder, 'README.md'), 'utf8')).toBe('# New 🦀\n')
      const creationStatus = await client.detailedRepositoryStatus(existingFolder, true, {}, true)
      expect(creationStatus.some(record => record.kind === 'entry' && Buffer.from(record.path.bytes).toString() === 'README.md')).toBe(true)
      expect((await client.repositoryStatusContext(existingFolder, true)).mergeHeadFound).toBe(false)
      await expect(client.detailedRepositoryStatus(existingFolder)).rejects.toThrow()
      await client.writeRepositoryCreationFile(existingFolder, 'gitignore', 'node_modules/\n')
      expect(await readFile(path.join(existingFolder, '.gitignore'), 'utf8')).toBe('node_modules/\n')
      await client.writeRepositoryCreationFile(existingFolder, 'license', 'license text\n')
      expect(await readFile(path.join(existingFolder, 'LICENSE'), 'utf8')).toBe('license text\n')
      await client.writeRepositoryCreationFile(existingFolder, 'git_attributes', 'first\n')
      await client.writeRepositoryCreationFile(existingFolder, 'git_attributes', 'second\n')
      expect(await readFile(path.join(existingFolder, '.gitattributes'), 'utf8')).toBe('first\n')
      await unlink(path.join(existingFolder, 'LICENSE'))
      await symlink(path.join(existingFolder, 'keep.txt'), path.join(existingFolder, 'LICENSE'))
      await expect(client.writeRepositoryCreationFile(existingFolder, 'license', 'unsafe')).rejects.toThrow()
      expect(await readFile(path.join(existingFolder, 'keep.txt'), 'utf8')).toBe('existing content')
      await expect(client.writeRepositoryCreationFile(existingFolder, 'readme', 'x'.repeat(65536))).rejects.toThrow('request limit')
      const nestedCreation = path.join(existingFolder, 'nested-creation')
      await mkdir(nestedCreation)
      await expect(client.writeRepositoryCreationFile(nestedCreation, 'readme', 'unsafe')).rejects.toThrow()
      await expect(client.detailedRepositoryStatus(nestedCreation, true, {}, true)).rejects.toThrow()
      await expect(client.repositorySafeFetchRemotes(nestedCreation)).resolves.toEqual([])
      await expect(client.initializeDesktopRepository(existingFolder, 'other')).rejects.toThrow()
      expect(await readFile(path.join(existingFolder, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/trunk\n')
      execFileSync('git', ['-C', existingFolder, 'config', 'user.name', 'Creation Test'])
      execFileSync('git', ['-C', existingFolder, 'config', 'user.email', 'creation@example.invalid'])
      execFileSync('git', ['-C', existingFolder, 'config', 'author.name', 'License Author'])
      execFileSync('git', ['-C', existingFolder, 'config', 'author.email', 'author@example.invalid'])
      expect(await client.repositoryEffectiveGitIdentity(existingFolder, 'author')).toMatch(/^Environment Author <environment@example\.invalid> \d+ /)
      expect(await client.repositoryEffectiveGitIdentity(existingFolder, 'committer')).toMatch(/^Creation Test <creation@example\.invalid> \d+ /)
      await expect(client.repositoryEffectiveGitIdentity(nestedCreation, 'author')).rejects.toThrow()
      await expect(client.request('git_repository_effective_identity', { path: existingFolder, role: 'GIT_EDITOR' })).rejects.toThrow()
      await expect(client.createInitialCommit(existingFolder, ['../keep.txt'])).rejects.toThrow()
      await client.createInitialCommit(existingFolder, ['README.md', '.gitattributes'])
      expect(execFileSync('git', ['-C', existingFolder, 'show', '--format=%s', '--no-patch', 'HEAD'], { encoding: 'utf8' })).toBe('Initial commit\n')
      expect(execFileSync('git', ['-C', existingFolder, 'ls-tree', '--name-only', 'HEAD'], { encoding: 'utf8' }).trim().split('\n')).toEqual(['.gitattributes', 'README.md'])
      await expect(client.createInitialCommit(existingFolder, ['keep.txt'])).rejects.toThrow()
      const nestedNew = path.join(directory, 'new parent', 'new child')
      await client.initializeDesktopRepository(nestedNew, 'feature/main')
      expect(await readFile(path.join(nestedNew, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/feature/main\n')
      await writeFile(path.join(nestedNew, 'staged'), 'preserve')
      await writeFile(path.join(nestedNew, 'unstaged'), 'not staged')
      execFileSync('git', ['-C', nestedNew, 'add', 'staged'])
      await expect(client.createInitialCommit(nestedNew, ['unstaged'])).rejects.toThrow()
      expect(execFileSync('git', ['-C', nestedNew, 'diff', '--cached', '--name-only'], { encoding: 'utf8' })).toBe('staged\n')
      const failedCommit = path.join(directory, 'failed commit')
      await client.initializeDesktopRepository(failedCommit, 'trunk')
      execFileSync('git', ['-C', failedCommit, 'config', 'user.name', 'Creation Test'])
      execFileSync('git', ['-C', failedCommit, 'config', 'user.email', 'creation@example.invalid'])
      await writeFile(path.join(failedCommit, 'file'), 'staged on failure')
      const hook = path.join(failedCommit, '.git', 'hooks', 'pre-commit')
      await writeFile(hook, '#!/bin/sh\nexit 1\n')
      await chmod(hook, 0o755)
      await expect(client.createInitialCommit(failedCommit, ['file'])).rejects.toMatchObject({ code: 'initial_commit_failed', retryable: false })
      expect(execFileSync('git', ['-C', failedCommit, 'diff', '--cached', '--name-only'], { encoding: 'utf8' })).toBe('file\n')
      const filteredCommit = path.join(directory, 'filtered commit')
      await client.initializeDesktopRepository(filteredCommit, 'trunk')
      execFileSync('git', ['-C', filteredCommit, 'config', 'user.name', 'Creation Test'])
      execFileSync('git', ['-C', filteredCommit, 'config', 'user.email', 'creation@example.invalid'])
      execFileSync('git', ['-C', filteredCommit, 'config', 'filter.probe.clean', 'test -z "$GIT_LITERAL_PATHSPECS" && cat'])
      execFileSync('git', ['-C', filteredCommit, 'config', 'filter.probe.required', 'true'])
      await writeFile(path.join(filteredCommit, '.gitattributes'), '*.txt filter=probe\n')
      await writeFile(path.join(filteredCommit, 'file.txt'), 'filtered content')
      await client.createInitialCommit(filteredCommit, ['.gitattributes', 'file.txt'])
      expect(execFileSync('git', ['-C', filteredCommit, 'show', 'HEAD:file.txt'], { encoding: 'utf8' })).toBe('filtered content')
      const insideRepository = path.join(firstRepository, 'nested project')
      await client.initializeDesktopRepository(insideRepository, 'nested')
      expect(await readFile(path.join(insideRepository, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/nested\n')
      await rm(insideRepository, { recursive: true, force: true })
      const blockedFolder = path.join(directory, 'blocked')
      const protectedFolder = path.join(directory, 'protected')
      await mkdir(blockedFolder)
      await mkdir(protectedFolder)
      await writeFile(path.join(protectedFolder, 'keep'), 'protected')
      await symlink(protectedFolder, path.join(blockedFolder, '.git'))
      await expect(client.initializeDesktopRepository(blockedFolder, 'trunk')).rejects.toThrow()
      expect(await readFile(path.join(protectedFolder, 'keep'), 'utf8')).toBe('protected')
      const invalidNew = path.join(directory, 'invalid branch')
      await expect(client.initializeDesktopRepository(invalidNew, 'bad..branch')).rejects.toThrow()
      expect(await client.directoryExists(invalidNew)).toBe(false)
      await expect(client.initializeDesktopRepository(bare, 'trunk')).rejects.toThrow()
      await expect(client.initializeDesktopRepository(path.parse(directory).root, 'trunk')).rejects.toThrow()
      expect(await client.snapshot()).toEqual(initialSelection)
      execFileSync('git', ['-C', firstRepository, 'config', '--local', 'submodule.libs/nested.url', 'https://user:synthetic-secret@github.com/Owner/Repo.git?token=synthetic-secret'])
      await expect(client.repositorySubmoduleDisplayUrl(firstRepository, 'libs/nested')).resolves.toBe('https://github.com/owner/repo')
      await expect(client.repositorySubmoduleDisplayUrl(firstRepository, 'missing')).resolves.toBeNull()
      await expect(client.repositorySubmoduleDisplayUrl(firstRepository, '../outside')).rejects.toThrow()
      await expect(client.repositoryGitDescription(firstRepository)).resolves.toBe('')
      await writeFile(path.join(firstRepository, '.git', 'description'), '  Publish this repository\n')
      await expect(client.repositoryGitDescription(firstRepository)).resolves.toBe('  Publish this repository\n')
      await writeFile(path.join(directory, 'private-description'), 'private synthetic text')
      await unlink(path.join(firstRepository, '.git', 'description'))
      await symlink(path.join(directory, 'private-description'), path.join(firstRepository, '.git', 'description'))
      await expect(client.repositoryGitDescription(firstRepository)).resolves.toBe('')
      await expect(client.setRepositoryGitDescription(firstRepository, 'unsafe')).rejects.toThrow()
      expect(await readFile(path.join(directory, 'private-description'), 'utf8')).toBe('private synthetic text')
      await unlink(path.join(firstRepository, '.git', 'description'))
      await writeFile(path.join(firstRepository, '.git', 'description'), '  Publish this repository\n')
      const unselected = path.join(directory, 'unselected')
      execFileSync('git', ['init', '-q', unselected])
      await mkdir(path.join(unselected, 'nested'))
      await writeFile(path.join(unselected, '.gitattributes'), '*.bin filter=lfs\n')
      execFileSync('git', ['-C', unselected, 'config', '--local', 'core.safecrlf', 'warn'])
      execFileSync('git', ['-C', unselected, 'config', '--local', 'commit.gpgsign', 'yes'])
      await expect(client.repositoryGitReadSetting(unselected, 'core.safecrlf', true, false)).resolves.toBe('warn')
      await expect(client.repositoryGitReadSetting(unselected, 'core.autocrlf', false, false)).resolves.toBe('input')
      await expect(client.repositoryGitReadSetting(unselected, 'core.autocrlf', true, false)).resolves.toBeNull()
      await expect(client.repositoryGitReadSetting(unselected, 'commit.gpgsign', false, true)).resolves.toBe('true')
      await expect(client.repositoryGitReadSetting(unselected, 'pull.ff', true, false)).resolves.toBeNull()
      await expect(client.request('git_read_repository_setting', { path: unselected, key: 'credential.helper' })).rejects.toThrow()
      await expect(client.request('git_read_repository_setting', { path: unselected, key: 'core.safecrlf', canonical_bool: true })).rejects.toThrow()
      const selectedBefore = await client.snapshot()
      await client.setRepositoryGitDescription(unselected, 'unselected description 🦀\n')
      expect(await readFile(path.join(unselected, '.git', 'description'), 'utf8')).toBe('unselected description 🦀\n')
      await expect(client.setRepositoryGitDescription(path.join(unselected, 'nested'), 'bad')).rejects.toThrow()
      await expect(client.repositoryLfsFileTracking(unselected, ['file.bin', 'plain'])).resolves.toEqual([true, false])
      expect(await client.repositoryUsesLfs(unselected)).toBe(await client.lfsAvailable())
      await expect(client.repositoryLfsFileTracking(path.join(unselected, 'nested'), ['file.bin'])).rejects.toThrow()
      await expect(client.repositoryLfsFileTracking('relative', ['file.bin'])).rejects.toThrow()
      await expect(client.request('lfs_repository', { path: unselected, action: { type: 'install_hooks', force: true, extra: true } })).rejects.toThrow()
      if (await client.lfsAvailable()) {
        const hook = path.join(unselected, '.git', 'hooks', 'pre-push')
        await mkdir(path.dirname(hook), { recursive: true })
        await writeFile(hook, '#!/bin/sh\necho unselected-custom-hook\n')
        await chmod(hook, 0o700)
        await expect(client.installRepositoryLfsHooks(unselected, false)).rejects.toMatchObject({ code: 'lfs_install_failed' })
        expect(await readFile(hook, 'utf8')).toContain('unselected-custom-hook')
        await client.installRepositoryLfsHooks(unselected, true)
        expect(await readFile(hook, 'utf8')).toContain('git lfs pre-push')
      }
      expect(await client.snapshot()).toEqual(selectedBefore)
      await writeFile(path.join(firstRepository, '.gitattributes'), '*.bin filter=lfs\n')
      expect(await client.repositoryUsesLfs(firstRepository)).toBe(await client.lfsAvailable())
      await expect(client.repositoryLfsFileTracking(firstRepository, ['a.bin', '-odd.txt', 'a.bin'])).resolves.toEqual([true, false, true])
      await expect(client.repositoryLfsFileTracking(firstRepository, ['../outside'])).rejects.toThrow()
      await unlink(path.join(firstRepository, '.gitattributes'))
      if (await client.lfsAvailable()) {
        const hook = path.join(firstRepository, '.git', 'hooks', 'pre-push')
        await mkdir(path.dirname(hook), { recursive: true })
        await writeFile(hook, '#!/bin/sh\necho custom-test-hook\n')
        await chmod(hook, 0o700)
        await expect(client.installRepositoryLfsHooks(firstRepository, false)).rejects.toMatchObject({ code: 'lfs_install_failed' })
        expect(await readFile(hook, 'utf8')).toContain('custom-test-hook')
        await client.installRepositoryLfsHooks(firstRepository, true)
        expect(await readFile(hook, 'utf8')).toContain('git lfs pre-push')
        await unlink(hook)
      }
      expect(await client.repositoryGitIdentityValue(firstRepository, 'user.name', true)).toBeNull()
      expect(await client.repositoryGitIdentityValue(firstRepository, 'user.name', false)).toBe('Bridge Test')
      await client.setRepositoryGitIdentityValue(firstRepository, 'user.name', 'Local Author')
      expect(await client.repositoryGitIdentityValue(firstRepository, 'user.name', true)).toBe('Local Author')
      await client.setRepositoryGitIdentityValue(firstRepository, 'user.name', null)
      await client.setRepositoryGitIdentityValue(firstRepository, 'user.name', null)
      expect(await client.repositoryGitIdentityValue(firstRepository, 'user.name', true)).toBeNull()
      expect(await client.repositoryGitIdentityValue(firstRepository, 'user.name', false)).toBe('Bridge Test')
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'commit',
        '--allow-empty',
        '-m',
        'initial',
      ])
      const firstSnapshot = (await bounded(client.snapshot())) as {
        root_id: string
      }
      await writeFile(path.join(firstRepository, 'untracked'), 'new file')
      const rawDiff = spawnSync(
        'git',
        ['diff', '--no-index', '--', '/dev/null', 'untracked'],
        { cwd: firstRepository, encoding: 'utf8' }
      )
      expect(rawDiff.status).toBe(1)
      const parsed = new DiffParser().parse(rawDiff.stdout)
      const displayed = {
        kind: DiffType.Text as const,
        text: parsed.contents,
        hunks: parsed.hunks,
        maxLineNumber: parsed.maxLineNumber,
        hasHiddenBidiChars: parsed.hasHiddenBidiChars,
      }
      const file = new WorkingDirectoryFileChange(
        'untracked',
        { kind: AppFileStatusKind.Untracked },
        DiffSelection.fromInitialSelection(
          DiffSelectionType.None
        ).withLineSelection(1, true)
      )
      const preview = await bounded(
        client.copilotCommitDiffForRepository(firstRepository, file.path)
      )
      expect(preview.path).toEqual({ bytes: [...Buffer.from(file.path)] })
      expect(getReviewedCopilotLines(file, displayed, preview)).toEqual([0])
      await writeFile(
        path.join(firstRepository, '.git', 'SQUASH_MSG'),
        'squash'
      )
      expect(
        await bounded(client.repositoryStatusContext(firstRepository))
      ).toMatchObject({
        mergeHeadFound: false,
        squashMsgFound: true,
        isCherryPickingHeadFound: false,
        rebaseInternalState: null,
      })
      const records = await bounded(
        client.detailedRepositoryStatus(firstRepository)
      )
      expect(records).toContainEqual(
        expect.objectContaining({ kind: 'entry', entry_kind: '?' })
      )
      await unlink(path.join(firstRepository, '.git', 'SQUASH_MSG'))
      expect(
        await bounded(
          client.repositoryBranches(firstRepository, ['refs/heads'])
        )
      ).toHaveLength(1)
      const alias = path.join(directory, 'repository alias')
      await symlink(firstRepository, alias, 'dir')
      await expect(client.directoryExists(alias)).resolves.toBe(true)
      await expect(
        client.directoryExists(path.join(firstRepository, '.git', 'HEAD'))
      ).resolves.toBe(false)
      await expect(
        client.pathExists(path.join(firstRepository, '.git', 'HEAD'))
      ).resolves.toBe(true)
      await mkdir(path.join(firstRepository, 'nested'))
      await expect(
        client.canonicalMutationPath(path.join(alias, 'nested', 'new', 'deep'))
      ).resolves.toBe(path.join(firstRepository, 'nested', 'new', 'deep'))
      const nestedAlias = path.join(directory, 'nested alias')
      await symlink(path.join(firstRepository, 'nested'), nestedAlias, 'dir')
      await expect(
        client.canonicalMutationPath(`${nestedAlias}/../.git/HEAD`)
      ).resolves.toBe(path.join(firstRepository, '.git', 'HEAD'))
      await expect(
        client.canonicalMutationPath(`${directory}/absent/.`)
      ).resolves.toBe(directory)
      await expect(
        client.canonicalMutationPath(
          path.join(firstRepository, '.git', 'HEAD', 'child')
        )
      ).rejects.toBeDefined()
      await expect(
        client.repositoryType(path.join(alias, 'nested'))
      ).resolves.toEqual({
        kind: 'regular',
        topLevelWorkingDirectory: alias,
        gitDir: path.join(firstRepository, '.git'),
      })
      expect(
        await client.repositoryBranches(alias, ['refs/heads'])
      ).toHaveLength(1)
      await expect(
        client.repositoryBranches(path.join(firstRepository, '.git'), [
          'refs/heads',
        ])
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryBranches('relative', ['refs/heads'])
      ).rejects.toMatchObject({ code: 'repository_changed' })
      execFileSync('git', ['-C', firstRepository, 'branch', 'topic'])
      const pointed = await bounded(
        client.branchesPointingAt(firstRepository, 'HEAD')
      )
      expect(pointed).toContain('topic')
      expect(pointed).toHaveLength(2)
      await expect(
        client.branchesPointingAt(firstRepository, 'missing')
      ).resolves.toBeNull()
      expect(
        await bounded(client.mergedRepositoryBranches(firstRepository, 'HEAD'))
      ).toEqual(
        expect.arrayContaining([
          expect.objectContaining({ ref: 'refs/heads/topic' }),
        ])
      )
      await expect(
        client.repositoryAheadBehind(firstRepository, 'HEAD...HEAD')
      ).resolves.toEqual({ ahead: 0, behind: 0 })
      await expect(
        client.repositoryAheadBehind(firstRepository, 'missing...HEAD')
      ).resolves.toBeNull()
      await expect(
        client.repositoryRevisionList(firstRepository, 'HEAD')
      ).resolves.toContain('initial')
      await expect(
        client.repositoryRevisionList(firstRepository, 'missing..HEAD')
      ).resolves.toBeNull()
      await expect(
        client.repositoryHasMergeCommits(firstRepository, 'HEAD')
      ).resolves.toBe(false)
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', 'topic'])
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'commit',
        '--allow-empty',
        '-m',
        'topic commit',
      ])
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', '-'])
      await expect(
        client.repositoryAheadBehind(firstRepository, 'HEAD...topic')
      ).resolves.toEqual({ ahead: 0, behind: 1 })
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'merge',
        '--no-ff',
        '-m',
        'merge topic',
        'topic',
      ])
      await expect(
        client.repositoryHasMergeCommits(firstRepository, 'HEAD')
      ).resolves.toBe(true)
      await writeFile(path.join(firstRepository, 'conflict.txt'), 'base\n')
      await writeFile(
        path.join(firstRepository, 'binary.dat'),
        Buffer.from([0, 1])
      )
      await writeFile(
        path.join(firstRepository, '.gitattributes'),
        'binary.dat merge=binary\n'
      )
      execFileSync('git', [
        '-C',
        firstRepository,
        'add',
        '--',
        'conflict.txt',
        'binary.dat',
        '.gitattributes',
      ])
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'commit',
        '-m',
        'base',
      ])
      execFileSync('git', [
        '-C',
        firstRepository,
        'switch',
        '-qc',
        'conflicting',
      ])
      await writeFile(path.join(firstRepository, 'conflict.txt'), 'side\n')
      await writeFile(
        path.join(firstRepository, 'binary.dat'),
        Buffer.from([0, 2])
      )
      execFileSync('git', [
        '-C',
        firstRepository,
        'add',
        '--',
        'conflict.txt',
        'binary.dat',
      ])
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'commit',
        '-m',
        'side',
      ])
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', '-'])
      await writeFile(path.join(firstRepository, 'conflict.txt'), 'main\n')
      await writeFile(
        path.join(firstRepository, 'binary.dat'),
        Buffer.from([0, 3])
      )
      execFileSync('git', [
        '-C',
        firstRepository,
        'add',
        '--',
        'conflict.txt',
        'binary.dat',
      ])
      execFileSync('git', [
        '-C',
        firstRepository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.com',
        'commit',
        '-m',
        'main',
      ])
      expect(() =>
        execFileSync(
          'git',
          [
            '-C',
            firstRepository,
            '-c',
            'user.name=Test',
            '-c',
            'user.email=test@example.com',
            'merge',
            'conflicting',
          ],
          { stdio: 'pipe' }
        )
      ).toThrow()
      const conflict = await client.repositoryConflictDetails(
        firstRepository,
        'merge'
      )
      expect(conflict.check).toContain('conflict.txt:')
      expect(conflict.check).toContain('leftover conflict marker')
      expect(conflict.attributes).toContain('conflict.txt\0merge\0')
      expect(conflict.attributes).toContain('binary.dat\0merge\0binary\0')
      await bounded(
        client.request('init_repository', { path: secondRepository })
      )
      await expect(
        client.repositoryGitIdentityValue(firstRepository, 'user.name', true)
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.setRepositoryGitIdentityValue(firstRepository, 'user.name', 'Wrong Repository')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      expect(spawnSync('git', ['-C', firstRepository, 'config', '--local', '--get', 'user.name']).status).toBe(1)
      await expect(
        client.repositoryBranches(firstRepository, ['refs/heads'])
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryBranches(alias, ['refs/heads'])
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryStatusContext(firstRepository)
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.detailedRepositoryStatus(firstRepository)
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryConflictDetails(firstRepository, 'merge')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.branchesPointingAt(firstRepository, 'HEAD')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.mergedRepositoryBranches(firstRepository, 'HEAD')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryAheadBehind(firstRepository, 'HEAD...HEAD')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryRevisionList(firstRepository, 'HEAD')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.repositoryHasMergeCommits(firstRepository, 'HEAD')
      ).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(
        client.request(
          'branch_enumeration',
          { prefixes: ['refs/heads'] },
          { repositoryId: firstSnapshot.root_id }
        )
      ).rejects.toThrow(/Repository selection changed/)

      const event = bounded(
        new Promise<unknown>(resolve => {
          const unsubscribe = client.subscribe('preferences_changed', value => {
            unsubscribe()
            resolve(value)
          })
        })
      )
      await bounded(
        client.request('update_preferences', { preferences: { theme: 'dark' } })
      )
      expect(await event).toMatchObject({ theme: 'dark' })

      const abort = new AbortController()
      const active = client.request(
        'github',
        { request: { action: { type: 'auth_status' } } },
        { signal: abort.signal }
      )
      const cancelled = expect(active).rejects.toMatchObject({
        code: 'cancelled',
        retryable: false,
      })
      await waitFor(
        async () => (await readFile(marker, 'utf8').catch(() => '')).length > 0
      )
      abort.abort()
      await cancelled
      // This request shares the session queue and can complete only after the
      // cancelled gh operation has finished its cleanup.
      await expect(
        bounded(client.request('get_preferences', {}))
      ).resolves.toMatchObject({ theme: 'dark' })

      const interrupted = client.request('github', {
        request: { action: { type: 'auth_status' } },
      })
      const disconnected = expect(interrupted).rejects.toMatchObject({
        code: 'disconnected',
        retryable: false,
      })
      await waitFor(
        async () =>
          (await readFile(marker, 'utf8')).trim().split('\n').length === 2
      )
      sockets[0].close()
      await disconnected
      await waitFor(() => !client.isReady)
      await bounded(client.waitReady())
      expect(sockets).toHaveLength(2)
      await expect(
        bounded(client.request('get_preferences', {}))
      ).resolves.toMatchObject({ theme: 'dark' })
      expect((await readFile(marker, 'utf8')).trim().split('\n')).toHaveLength(
        2
      )

      const restored = new BackendClient()
      clients.push(restored)
      expect(restored.transportKind).toBe('ws')
      expect((await bounded(restored.waitReady())).ready).toBe(true)
      restored.close()
      dom.window.sessionStorage.setItem('desktopBridgeToken', 'wrong-token')
      const invalid = new BackendClient()
      clients.push(invalid)
      await expect(bounded(invalid.waitReady())).rejects.toMatchObject({
        code: 'unauthorized',
        retryable: false,
      })
      expect(dom.window.sessionStorage.getItem('desktopBridgeToken')).toBeNull()
    }, 40_000)

    it('initializes the requested directory even when Git environment points elsewhere', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-init-env-'))
      const decoy = path.join(directory, 'decoy')
      const destination = path.join(directory, 'destination')
      execFileSync('git', ['init', '-q', decoy])
      const originalHead = await readFile(path.join(decoy, '.git', 'HEAD'), 'utf8')
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), [
        '--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0',
      ], {
        env: {
          ...process.env,
          HOME: directory,
          GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
          GIT_CONFIG_NOSYSTEM: '1',
          GIT_DIR: path.join(decoy, '.git'),
          GIT_WORK_TREE: decoy,
          GIT_COMMON_DIR: path.join(decoy, '.git'),
          GIT_INDEX_FILE: path.join(decoy, '.git', 'index'),
          GIT_OBJECT_DIRECTORY: path.join(decoy, '.git', 'objects'),
        },
        stdio: ['ignore', 'ignore', 'pipe'],
      })
      const lines = createInterface({ input: server.stderr! })
      const announcement = await bounded(new Promise<string>((resolve, reject) => {
        lines.once('line', resolve)
        server!.once('error', reject)
        server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`)))
      }))
      const sessionUrl = announcement.startsWith('Open ') ? announcement.slice(5) : ''
      expect(/^http:\/\/127\.0\.0\.1:\d+\/#token=/.test(sessionUrl)).toBe(true)
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window)
      vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
      vi.stubGlobal('WebSocket', dom.window.WebSocket)
      const client = new BackendClient()
      clients.push(client)
      await bounded(client.waitReady())
      await client.initializeDesktopRepository(destination, 'trunk')
      expect(await readFile(path.join(destination, '.git', 'HEAD'), 'utf8')).toBe('ref: refs/heads/trunk\n')
      expect(await readFile(path.join(decoy, '.git', 'HEAD'), 'utf8')).toBe(originalHead)
      await expect(client.snapshot()).resolves.toBeNull()
    }, 30_000)
  }
)
