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
        path.join(directory, 'gitconfig'),
        '[user]\n\tname = Bridge Test\n\temail = bridge@example.invalid\n'
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
            GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
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
      expect(await bounded(client.gitVersion())).toBe(execFileSync('git', ['--version'], { encoding: 'utf8' }).trim().replace(/^git version /, ''))
      expect(await bounded(client.request('git_settings', { scope: 'global' }))).toMatchObject({
        scope: 'global',
        values: { 'user.name': 'Bridge Test', 'user.email': 'bridge@example.invalid' },
      })
      expect(await bounded(client.request('set_git_setting', { scope: 'global', key: 'user.name', value: '' }))).toMatchObject({ values: { 'user.name': '', 'user.email': 'bridge@example.invalid' } })
      expect(await bounded(client.request('git_settings', { scope: 'global' }))).toMatchObject({ values: { 'user.name': '' } })
      await expect(bounded(client.request('set_git_setting', { scope: 'global', key: 'credential.helper', value: 'unsafe' }))).rejects.toThrow()
      expect(await bounded(client.request('set_git_setting', { scope: 'global', key: 'user.name', value: 'Bridge Test' }))).toMatchObject({ values: { 'user.name': 'Bridge Test' } })
      expect(dom.window.location.hash).toBe('')
      await expect(bounded(client.snapshot())).resolves.toBeNull()
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
  }
)
