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
  readdir,
  rm,
  writeFile,
  chmod,
  unlink,
  symlink,
  rename,
  open,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { createRequire } from 'node:module'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'
import type { Repository } from '../../src/models/repository'
import { getReviewedCopilotLines, getReviewedDiffLines } from '../../src/lib/copilot/commit-selection'
import { DiffParser } from '../../src/lib/diff-parser'
import {
  DiffSelection,
  DiffSelectionType,
  DiffType,
} from '../../src/models/diff'
import {
  AppFileStatusKind,
  FileChange,
  WorkingDirectoryFileChange,
} from '../../src/models/status'

const enabled = process.env.DESKTOP_TEST_BRIDGE === '1'
const route = vi.hoisted(() => ({ client: undefined as BackendClient | undefined }))
vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../src/lib/custom-integration', () => ({}))
vi.mock('../../src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../src/backend/client')>()),
  backend: () => {
    if (!route.client) throw new Error('No integration backend selected')
    return route.client
  },
}))
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
  route.client = undefined
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
    it('reads last fetch time from the selected repository', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-last-fetch-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      dom = new JSDOM('', { url: line.startsWith('Open ') ? line.slice(5) : '' })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket); vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady()); route.client = client
      const repository = path.join(directory, 'repository'); const remote = path.join(directory, 'remote.git')
      execFileSync('git', ['init', '--bare', '-q', remote]); await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      await expect(client.repositoryLastFetch(repository)).resolves.toEqual({ kind: 'unavailable' })
      await writeFile(path.join(repository, '.git', 'FETCH_HEAD'), '')
      await expect(client.repositoryLastFetch(repository)).resolves.toEqual({ kind: 'empty' })
      // Git writes FETCH_HEAD only after an actual remote reference is fetched.
      execFileSync('git', ['-C', repository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-qm', 'initial'])
      execFileSync('git', ['-C', repository, 'push', '-q', remote, 'HEAD:refs/heads/main'])
      execFileSync('git', ['-C', repository, 'fetch', '-q', remote, 'refs/heads/main'])
      const fetched = await client.repositoryLastFetch(repository)
      expect(fetched.kind).toBe('fetched')
      if (fetched.kind === 'fetched') expect(Math.abs(Date.now() - fetched.time)).toBeLessThan(30_000)
    }, 90_000)

    it('reviews and recovers remote and combined branch deletions', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-delete-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      dom = new JSDOM('', { url: line.startsWith('Open ') ? line.slice(5) : '' })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket); vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady()); route.client = client
      const repository = path.join(directory, 'repository'); const first = path.join(directory, 'first.git'); const second = path.join(directory, 'second.git')
      execFileSync('git', ['init', '--bare', '-q', first]); execFileSync('git', ['init', '--bare', '-q', second])
      await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      const git = (...args: string[]) => execFileSync('git', ['-C', repository, ...args], { encoding: 'utf8' }).trim()
      git('config', 'user.name', 'Test'); git('config', 'user.email', 'test@example.invalid'); git('config', 'commit.gpgsign', 'false'); git('commit', '--allow-empty', '-qm', 'initial'); git('branch', '-m', 'main')
      git('remote', 'add', 'origin', first); git('remote', 'set-url', '--add', '--push', 'origin', first); git('remote', 'set-url', '--add', '--push', 'origin', second)
      for (const remote of [first, second]) git('push', '-q', remote, 'HEAD:refs/heads/topic')
      const review = await client.previewRemoteBranchDeletion(repository, 'origin', 'topic')
      expect(review.endpoints).toHaveLength(2)
      const { receipt } = await client.applyRemoteBranchDeletion(repository, review)
      git('config', '--unset-all', 'remote.origin.pushurl')
      git('config', '--add', 'remote.origin.pushurl', path.join(directory, 'changed.git'))
      const history = await client.remoteBranchDeletionHistory(repository)
      expect(history.find(record => record.id === receipt)?.endpoints.map(endpoint => endpoint.phase)).toEqual(['deleted', 'deleted'])
      expect(history.find(record => record.id === receipt)?.endpoints.map(endpoint => endpoint.destination)).toEqual([first, second])
      await client.restoreRemoteBranchTip(repository, receipt, 0, 'recovered-topic')
      expect(git('rev-parse', 'refs/heads/recovered-topic')).toBe(git('rev-parse', 'HEAD'))
      git('config', '--unset-all', 'remote.origin.pushurl')
      git('config', '--add', 'remote.origin.pushurl', first); git('config', '--add', 'remote.origin.pushurl', second)
      git('checkout', '-qb', 'feature')
      const tip = git('rev-parse', 'HEAD')
      for (const remote of [first, second]) git('push', '-q', remote, 'HEAD:refs/heads/feature')
      git('config', 'branch.feature.remote', 'origin'); git('config', 'branch.feature.merge', 'refs/heads/feature')
      const combined = await client.previewCombinedBranchDeletion(repository, 'feature', tip, 'main', 'origin', 'feature')
      expect(combined.endpoints).toHaveLength(2)
      await client.applyRemoteBranchDeletion(repository, combined)
      expect(git('symbolic-ref', '--short', 'HEAD')).toBe('main')
      expect(spawnSync('git', ['-C', repository, 'show-ref', '--verify', '--quiet', 'refs/heads/feature']).status).not.toBe(0)
    }, 90_000)

    it('reads bounded working images through the Rust bridge', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-image-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket); vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady()); route.client = client
      const repository = path.join(directory, 'repository')
      await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      const imagePath = path.join(repository, '[a].PNG')
      const bytes = Buffer.alloc(2 * 1024 * 1024 + 23, 0xab); bytes[0] = 0; bytes[1] = 255
      await writeFile(imagePath, bytes)
      const { getWorkingDirectoryImage } = await import('../../src/lib/git/diff')
      const model = { path: repository } as Repository
      const image = await getWorkingDirectoryImage(model, new FileChange('[a].PNG', { kind: AppFileStatusKind.Modified }))
      expect(Buffer.from(image.rawContents)).toEqual(bytes)
      expect(image.contents).toBe(bytes.toString('base64'))
      expect(image.mediaType).toBe('image/png')
      await expect(getWorkingDirectoryImage(model, new FileChange('missing.png', { kind: AppFileStatusKind.Modified }))).rejects.toThrow()
      await symlink('[a].PNG', path.join(repository, 'link.png'))
      const linked = await getWorkingDirectoryImage(model, new FileChange('link.png', { kind: AppFileStatusKind.Modified }))
      expect(Buffer.from(linked.rawContents)).toEqual(bytes)
      await symlink('/etc/passwd', path.join(repository, 'outside.png'))
      await expect(getWorkingDirectoryImage(model, new FileChange('outside.png', { kind: AppFileStatusKind.Modified }))).rejects.toThrow()
      const huge = await open(path.join(repository, 'huge.png'), 'w'); await huge.truncate(70_000_001); await huge.close()
      await expect(getWorkingDirectoryImage(model, new FileChange('huge.png', { kind: AppFileStatusKind.Modified }))).rejects.toThrow()
    }, 90_000)

    it('reviews worktree lifecycle and relocates the selected saved record', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-worktree-bridge-'))
      const registryPath = path.join(directory, 'registry.json')
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', registryPath, 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket); vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady()); route.client = client
      const main = path.join(directory, 'main'); const linked = path.join(directory, 'linked'); const moved = path.join(directory, 'moved')
      await client.request('init_repository', { path: main }); await client.selectBrowserRepository(main)
      const git = (...args: string[]) => execFileSync('git', ['-C', main, ...args], { encoding: 'utf8' }).trim()
      git('config', 'user.name', 'Test'); git('config', 'user.email', 'test@example.invalid'); git('config', 'commit.gpgsign', 'false'); git('commit', '--allow-empty', '-qm', 'initial')
      await client.mutateRepositoryWorktree(main, { kind: 'add', path: linked, create_branch: 'feature', commitish: null })
      await client.selectBrowserRepository(linked)
      const before = JSON.parse(await readFile(registryPath, 'utf8'))
      const saved = before.records.find((entry: { path: string }) => entry.path === linked)
      expect(saved).toBeDefined()
      await client.mutateRepositoryWorktree(linked, { kind: 'move', from: linked, to: moved })
      const after = JSON.parse(await readFile(registryPath, 'utf8'))
      expect(after.records.find((entry: { id: string }) => entry.id === saved.id).path).toBe(moved)
      await client.selectBrowserRepository(main)
      await writeFile(path.join(moved, 'untracked'), 'keep')
      await expect(client.mutateRepositoryWorktree(main, { kind: 'remove', path: moved, force: false })).rejects.toMatchObject({ code: 'git_error' })
      expect(await readFile(path.join(moved, 'untracked'), 'utf8')).toBe('keep')
      await client.mutateRepositoryWorktree(main, { kind: 'remove', path: moved, force: true })
      await expect(readFile(path.join(moved, 'untracked'))).rejects.toMatchObject({ code: 'ENOENT' })
    }, 90_000)

    it('renders comparison diffs through the Rust bridge', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-compare-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      expect(sessionUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/#token=/)
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket); vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady()); route.client = client
      const repository = path.join(directory, 'repository')
      await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      const git = (...args: string[]) => execFileSync('git', ['-C', repository, ...args], { maxBuffer: 8 * 1024 * 1024 })
      git('config', 'user.name', 'Test'); git('config', 'user.email', 'test@example.invalid'); git('config', 'commit.gpgsign', 'false')
      await writeFile(path.join(repository, '[a].txt'), 'first\n'); await writeFile(path.join(repository, 'a.txt'), 'unrelated\n')
      await writeFile(path.join(repository, 'old name.txt'), 'renamed content\n')
      const binaryBlob = Buffer.alloc(1024 * 1024 + 3, Buffer.from([0, 255, 1]))
      await writeFile(path.join(repository, 'binary.dat'), binaryBlob)
      git('add', '--', '.'); git('commit', '-qm', 'first')
      const first = git('rev-parse', 'HEAD').toString().trim()
      await writeFile(path.join(repository, '[a].txt'), `${'a'.repeat(120)}\n`.repeat(1200)); await writeFile(path.join(repository, 'a.txt'), 'must not appear\n')
      git('mv', 'old name.txt', 'new name.txt')
      await writeFile(path.join(repository, 'binary.dat'), Buffer.from([0, 9, 8]))
      git('add', '--', '.'); git('commit', '-qm', 'second')
      const last = git('rev-parse', 'HEAD').toString().trim()
      const raw = await client.repositoryRendererCompareDiff(repository, first, last, true, '[a].txt', undefined, false)
      expect(raw).not.toBeNull()
      const expected = git('diff', '--merge-base', '--patch-with-raw', '-z', '--no-color', '--end-of-options', first, last, '--', ':(literal)[a].txt')
      expect(expected.length).toBeGreaterThan(65_536)
      expect(Buffer.from(raw as Uint8Array)).toEqual(expected)
      const changes = await client.repositoryRendererCompareChanges(repository, first, last, true)
      expect(Buffer.from(changes as Uint8Array)).toEqual(git('diff', '--merge-base', '-C', '-M', '-z', '--raw', '--numstat', '--end-of-options', first, last, '--'))
      const rangeChanges = await client.repositoryRendererCompareChanges(repository, first, last, false)
      expect(Buffer.from(rangeChanges as Uint8Array)).toEqual(git('diff', '-C', '-M', '-z', '--raw', '--numstat', '--end-of-options', first, last, '--'))
      await expect(client.repositoryRendererCompareChanges(repository, `${first}^`, first, false)).resolves.toBeNull()
      const { getCommitRangeDiff, getBranchMergeBaseChangedFiles, getCommitRangeChangedFiles } = await import('../../src/lib/git/diff')
      const model = { path: repository, gitHubRepository: null } as Repository
      const { getBlobContents } = await import('../../src/lib/git/show')
      expect(await getBlobContents(model, first, 'binary.dat')).toEqual(binaryBlob)
      expect(await client.repositoryBlobSnapshot(repository, first, 'missing')).toBeNull()
      const listed = await getBranchMergeBaseChangedFiles(model, first, last, last)
      expect(listed?.files.find(file => file.path === 'new name.txt')?.status).toMatchObject({ kind: AppFileStatusKind.Renamed, oldPath: 'old name.txt' })
      expect(listed?.files.map(file => file.path)).toContain('binary.dat')
      const firstCommit = await getCommitRangeChangedFiles(model, [first])
      expect(firstCommit.files.map(file => file.path)).toContain('[a].txt')
      const diff = await getCommitRangeDiff({ path: repository, gitHubRepository: null } as Repository, new FileChange('[a].txt', { kind: AppFileStatusKind.Modified }), [first])
      expect(diff.kind).toBe(DiffType.Text)
      if (diff.kind === DiffType.Text) expect(diff.text).toContain('+first')
      await expect(client.repositoryRendererCompareDiff(repository, 'missing-branch', last, true, '[a].txt', undefined, false)).rejects.toThrow()
      await writeFile(path.join(repository, '[a].txt'), `${'x'.repeat(120)}\n`.repeat(20_000))
      git('add', '--', '.'); git('commit', '-qm', 'large diff')
      const large = git('rev-parse', 'HEAD').toString().trim()
      const largeRaw = await client.repositoryRendererCompareDiff(repository, last, large, false, '[a].txt', undefined, false)
      const largeExpected = git('diff', '--patch-with-raw', '-z', '--no-color', '--format=', '--end-of-options', last, large, '--', ':(literal)[a].txt')
      expect(largeExpected.length).toBeGreaterThan(2 * 1024 * 1024)
      expect(Buffer.from(largeRaw as Uint8Array)).toEqual(largeExpected)
      const commitRaw = await client.repositoryRendererCommitDiff(repository, large, '[a].txt', undefined, false)
      const commitExpected = git('--literal-pathspecs', 'log', '-m', '-1', '--first-parent', '--patch-with-raw', '--format=', '-z', '--no-color', '--end-of-options', large, '--', '[a].txt')
      expect(commitExpected.length).toBeGreaterThan(2 * 1024 * 1024)
      expect(Buffer.from(commitRaw as Uint8Array)).toEqual(commitExpected)
      const rootRaw = await client.repositoryRendererCommitDiff(repository, first, 'binary.dat', undefined, false)
      expect(Buffer.from(rootRaw as Uint8Array)).toEqual(git('--literal-pathspecs', 'log', '-m', '-1', '--first-parent', '--patch-with-raw', '--format=', '-z', '--no-color', '--end-of-options', first, '--', 'binary.dat'))
      const renameRaw = await client.repositoryRendererCommitDiff(repository, last, 'new name.txt', 'old name.txt', true)
      expect(Buffer.from(renameRaw as Uint8Array)).toEqual(git('--literal-pathspecs', 'log', '-w', '-m', '-1', '--first-parent', '--patch-with-raw', '--format=', '-z', '--no-color', '--end-of-options', last, '--', 'new name.txt', 'old name.txt'))
      const { getCommitDiff } = await import('../../src/lib/git/diff')
      const commitDiff = await getCommitDiff(model, new FileChange('[a].txt', { kind: AppFileStatusKind.Modified }), last)
      expect(commitDiff.kind).toBe(DiffType.Text)
      const mainBranch = git('symbolic-ref', '--short', 'HEAD').toString().trim()
      git('checkout', '-qb', 'resolution-side')
      await writeFile(path.join(repository, '[a].txt'), 'side version\n'); git('add', '--', '.'); git('commit', '-qm', 'side')
      git('checkout', '-q', mainBranch)
      await writeFile(path.join(repository, '[a].txt'), 'main version\n'); git('add', '--', '.'); git('commit', '-qm', 'main')
      expect(() => git('merge', 'resolution-side')).toThrow()
      await writeFile(path.join(repository, '[a].txt'), Buffer.from([0xef, 0xbb, 0xbf, 0x78, 0xff, 0x0a]))
      const { getResolutionDiff } = await import('../../src/lib/git/diff')
      const stage = await getResolutionDiff(model, '[a].txt', { stage: 'theirs' })
      expect(stage.oldContents).toBe('\ufeffx�\n')
      expect(stage.newContents).toBe('side version\n')
      const replacement = `${'new content\n'.repeat(8_000)}✓\n`
      const resolution = await getResolutionDiff(model, '[a].txt', { content: replacement })
      expect(resolution.oldContents).toBe('\ufeffx�\n')
      expect(resolution.newContents).toBe(replacement)
      expect(resolution.diff.kind).toBe(DiffType.Text)
      const binding = await client.request<{ root_id: string }>('repository_binding', { path: repository })
      const snapshot = await client.request<{ id: string }>('renderer_compare_diff', { before: last, after: large, merge_base: false, path: '[a].txt', old_path: null, hide_whitespace: false, snapshot: true }, { repositoryId: binding.root_id })
      await client.selectBrowserRepository(null)
      await expect(client.request('release_comparison_snapshot', { snapshot_id: snapshot.id })).resolves.toEqual({ released: true })
    }, 90_000)

    it('fetches a reviewed local remote over WebSocket', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-fetch-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      expect(sessionUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/#token=/)
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket)
      vi.stubGlobal('location', dom.window.location)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady())
      route.client = client
      const { fetch: fetchRemote, fetchRefspec } = await import('../../src/lib/git/fetch')
      const source = path.join(directory, 'source'); const repository = path.join(directory, 'repository')
      execFileSync('git', ['init', '-q', '-b', 'main', source])
      execFileSync('git', ['-C', source, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '--author=Test <test@example.invalid>', '-qm', 'base'])
      execFileSync('git', ['-C', source, '-c', 'user.name=Second', '-c', 'user.email=second@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '--author=Second <second@example.invalid>', '-qm', 'second'])
      await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      const git = (...args: string[]) => execFileSync('git', ['-C', repository, ...args], { encoding: 'utf8' }).trim()
      git('remote', 'add', '--', '--test', source)
      await expect(client.fetchRepositoryRemote(repository, '--test', '/wrong/path', false)).rejects.toThrow()
      const actionProgress = vi.fn()
      await fetchRemote({ path: repository, gitHubRepository: null } as Repository, { name: '--test', url: source }, actionProgress, true, false)
      expect(actionProgress).toHaveBeenCalledWith(expect.objectContaining({ kind: 'fetch', remote: '--test', value: 0 }))
      expect(actionProgress.mock.calls.some(([value]) => typeof value.description === 'string')).toBe(true)
      expect(git('rev-parse', 'refs/remotes/--test/main')).toBe(execFileSync('git', ['-C', source, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim())
      expect(await readFile(path.join(repository, '.git', 'FETCH_HEAD'), 'utf8')).toContain('branch')
      const repositoryModel = { path: repository, gitHubRepository: null } as Repository
      const { getAuthors } = await import('../../src/lib/git/log')
      const head = git('rev-parse', 'refs/remotes/--test/main')
      const previous = git('rev-parse', `${head}^`)
      expect((await getAuthors(repositoryModel, [head.toUpperCase(), previous])).map(author => author.name)).toEqual(['Second', 'Test'])
      await expect(getAuthors(repositoryModel, [head, head.toUpperCase()])).rejects.toThrow('Commit to author mismatch')
      await expect(getAuthors(repositoryModel, ['refs/remotes/--test/main'])).rejects.toThrow()
      const { updateRemoteHEAD } = await import('../../src/lib/git/remote')
      git('symbolic-ref', '--delete', 'refs/remotes/--test/HEAD')
      await updateRemoteHEAD(repositoryModel, { name: '--test', url: source }, true)
      expect(git('symbolic-ref', 'refs/remotes/--test/HEAD')).toBe('refs/remotes/--test/main')
      await fetchRefspec(repositoryModel, { name: '--test', url: source }, 'refs/heads/main:refs/remotes/--test/review')
      expect(git('rev-parse', 'refs/remotes/--test/review')).toBe(git('rev-parse', 'refs/remotes/--test/main'))
      await expect(fetchRefspec(repositoryModel, { name: '--test', url: source }, 'refs/heads/missing')).resolves.toBeUndefined()
      const missing = path.join(directory, 'missing-remote')
      git('remote', 'add', 'missing', missing)
      await expect(fetchRemote({ path: repository, gitHubRepository: null } as Repository, { name: 'missing', url: missing })).rejects.toMatchObject({ name: 'GitError', result: { exitCode: expect.any(Number) } })
    }, 90_000)

    it('fast-forwards reviewed tracking branches without touching FETCH_HEAD', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-ff-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      expect(sessionUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/#token=/)
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window); vi.stubGlobal('sessionStorage', dom.window.sessionStorage); vi.stubGlobal('WebSocket', dom.window.WebSocket)
      const client = new BackendClient(); clients.push(client); await bounded(client.waitReady())
      const repository = path.join(directory, 'repository')
      await client.request('init_repository', { path: repository }); await client.selectBrowserRepository(repository)
      const git = (...args: string[]) => execFileSync('git', ['-C', repository, ...args], { encoding: 'utf8' }).trim()
      git('config', 'user.name', 'Test'); git('config', 'user.email', 'test@example.invalid'); git('config', 'commit.gpgsign', 'false')
      await writeFile(path.join(repository, 'file'), 'base\n'); git('add', 'file'); git('commit', '-qm', 'base')
      const sha = git('rev-parse', 'HEAD'); git('branch', 'tracking', sha)
      await writeFile(path.join(repository, 'file'), 'new\n'); git('commit', '-qam', 'new'); const upstreamSha = git('rev-parse', 'HEAD')
      git('config', 'remote.origin.url', 'https://example.invalid/repo'); git('config', 'remote.origin.fetch', '+refs/heads/*:refs/remotes/origin/*'); git('config', 'branch.tracking.remote', 'origin'); git('config', 'branch.tracking.merge', 'refs/heads/topic'); git('update-ref', 'refs/remotes/origin/topic', upstreamSha)
      await writeFile(path.join(repository, '.git', 'FETCH_HEAD'), 'keep\n')
      const branches = [{ ref: 'refs/heads/tracking', sha, upstreamRef: 'refs/remotes/origin/topic', upstreamSha }]
      await expect(client.fastForwardRepositoryBranches(repository, [{ ...branches[0], sha: '0'.repeat(40) }], false)).rejects.toThrow()
      await client.fastForwardRepositoryBranches(repository, branches, false)
      expect(git('rev-parse', 'tracking')).toBe(upstreamSha)
      expect(await readFile(path.join(repository, '.git', 'FETCH_HEAD'), 'utf8')).toBe('keep\n')
    }, 90_000)

    it('performs reviewed mixed reset without changing working bytes', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-reset-bridge-'))
      server = spawn(path.resolve('rust/target/debug/desktop-rs'), ['--config', path.join(directory, 'registry.json'), 'serve', '--empty', '--port', '0'], { env: { ...process.env, HOME: directory, GIT_CONFIG_NOSYSTEM: '1' }, stdio: ['ignore', 'ignore', 'pipe'] })
      const lines = createInterface({ input: server.stderr! })
      const line = await bounded(new Promise<string>((resolve, reject) => { lines.once('line', resolve); server!.once('error', reject); server!.once('exit', code => reject(new Error(`bridge exited before startup: ${code}`))) }))
      const sessionUrl = line.startsWith('Open ') ? line.slice(5) : ''
      expect(sessionUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/#token=/)
      dom = new JSDOM('', { url: sessionUrl })
      vi.stubGlobal('window', dom.window)
      vi.stubGlobal('sessionStorage', dom.window.sessionStorage)
      vi.stubGlobal('WebSocket', dom.window.WebSocket)
      const client = new BackendClient()
      clients.push(client)
      await bounded(client.waitReady())
      const repository = path.join(directory, 'repository')
      await client.request('init_repository', { path: repository })
      await client.selectBrowserRepository(repository)
      execFileSync('git', ['-C', repository, 'config', 'commit.gpgsign', 'false'])
      await writeFile(path.join(repository, 'file'), 'base\n')
      execFileSync('git', ['-C', repository, 'add', 'file'])
      execFileSync('git', ['-C', repository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'base'])
      const target = execFileSync('git', ['-C', repository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(repository, 'file'), 'commit\n')
      execFileSync('git', ['-C', repository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qam', 'change'])
      const head = execFileSync('git', ['-C', repository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      const branch = execFileSync('git', ['-C', repository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(repository, 'file'), 'working\n')
      const request = { target, head, branch, undo: true }
      await expect(client.mixedResetRepository(repository, { ...request, target: head })).rejects.toThrow()
      await client.mixedResetRepository(repository, request)
      expect(execFileSync('git', ['-C', repository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(target)
      expect(execFileSync('git', ['-C', repository, 'show', ':file'], { encoding: 'utf8' })).toBe('base\n')
      expect(await readFile(path.join(repository, 'file'), 'utf8')).toBe('working\n')
    }, 90_000)

    // This journey exercises many real Git and bridge workflows; slower hosts
    // can exceed the smaller timeout even while every operation is progressing.
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
      await client.selectBrowserRepository(null)
      expect(await client.snapshot()).toBeNull()
      await expect(client.repositoryStatusContext(firstRepository)).rejects.toMatchObject({ code: 'repository_changed' })
      await client.selectBrowserRepository(firstRepository)
      await expect(client.repositoryRebaseState(firstRepository)).resolves.toBeNull()
      await writeFile(path.join(firstRepository, '.git', 'REBASE_HEAD'), 'b'.repeat(40) + '\n')
      await expect(client.repositoryRebaseState(firstRepository)).resolves.toBeNull()
      await mkdir(path.join(firstRepository, '.git', 'rebase-apply'))
      await expect(client.repositoryRebaseState(firstRepository)).resolves.toEqual({ head: 'b'.repeat(40), progress: null })
      await rm(path.join(firstRepository, '.git', 'rebase-apply'), { recursive: true })
      await unlink(path.join(firstRepository, '.git', 'REBASE_HEAD'))
      await expect(client.repositoryCherryPickState(firstRepository)).resolves.toBeNull()
      await writeFile(path.join(firstRepository, '.git', 'CHERRY_PICK_HEAD'), 'a'.repeat(40) + '\n')
      await expect(client.repositoryCherryPickState(firstRepository)).resolves.toEqual({ head: 'a'.repeat(40), sequence: null })
      await unlink(path.join(firstRepository, '.git', 'CHERRY_PICK_HEAD'))
      const largeFixture = await open(path.join(firstRepository, 'oversized'), 'w')
      await largeFixture.truncate(100 * 1024 * 1024 + 1)
      await largeFixture.close()
      const boundaryFixture = await open(path.join(firstRepository, 'boundary'), 'w')
      await boundaryFixture.truncate(100 * 1024 * 1024)
      await boundaryFixture.close()
      await symlink('oversized', path.join(firstRepository, 'large-link'))
      await expect(client.repositoryLargeFiles(firstRepository, ['oversized', 'boundary', 'large-link', 'missing'])).resolves.toEqual([true, false, false, false])
      await expect(client.repositoryLargeFiles(firstRepository, ['../outside'])).rejects.toThrow()
      const outsideSize = path.join(directory, 'outside-size')
      await mkdir(outsideSize)
      await writeFile(path.join(outsideSize, 'file'), 'private')
      await symlink(outsideSize, path.join(firstRepository, 'size-escape'))
      await expect(client.repositoryLargeFiles(firstRepository, ['size-escape/file'])).rejects.toThrow()
      await unlink(path.join(firstRepository, 'size-escape'))
      await unlink(path.join(firstRepository, 'oversized'))
      await unlink(path.join(firstRepository, 'boundary'))
      await unlink(path.join(firstRepository, 'large-link'))
      await expect(client.repositoryRendererTagRefs(firstRepository)).resolves.toBe('')
      await expect(client.repositoryRendererHistory(firstRepository, 'HEAD', 10, 0, false, false)).resolves.toBeNull()
      await expect(client.repositoryRendererReflog(firstRepository, 'branches')).resolves.toBeNull()
      await expect(client.repositoryRendererReflog(firstRepository, 'stash')).resolves.toBeNull()
      await expect(client.repositorySelectedCommitPreview(firstRepository, {})).rejects.toThrow()
      expect((await client.repositorySelectedCommitPreview(firstRepository, { allow_empty: true })).review.staged_count).toBe(0)
      await expect(client.repositoryBranchCheckouts(firstRepository, new Date('2000-01-01T00:00:00.000Z'))).resolves.toBeNull()
      await writeFile(path.join(firstRepository, 'staged\nname'), 'staged')
      execFileSync('git', ['-C', firstRepository, 'add', '--', 'staged\nname'])
      await expect(client.repositoryIndexChanges(firstRepository)).resolves.toEqual([{ path: 'staged\nname', status: 'A' }])
      execFileSync('git', ['-C', firstRepository, 'rm', '--cached', '-q', '--', 'staged\nname'])
      await unlink(path.join(firstRepository, 'staged\nname'))
      const invalidName = Buffer.concat([Buffer.from(`${firstRepository}/`), Buffer.from([0xff])])
      await writeFile(invalidName, 'staged')
      execFileSync('git', ['-C', firstRepository, 'add', '-A'])
      await expect(client.repositoryIndexChanges(firstRepository)).rejects.toMatchObject({ code: 'unsupported_path' })
      execFileSync('git', ['-C', firstRepository, 'rm', '--cached', '-r', '-q', '--', '.'])
      await unlink(invalidName)
      const initialSelection = await client.snapshot()
      const existingFolder = path.join(directory, 'existing folder')
      await mkdir(existingFolder)
      await writeFile(path.join(existingFolder, 'keep.txt'), 'existing content')
      await client.initializeDesktopRepository(existingFolder, 'trunk')
      expect(await client.repositoryGitIgnore(existingFolder, { type: 'read' })).toBeNull()
      await client.repositoryGitIgnore(existingFolder, { type: 'save', text: 'first' })
      await client.repositoryGitIgnore(existingFolder, { type: 'append', text: 'second' })
      expect(await client.repositoryGitIgnore(existingFolder, { type: 'read' })).toBe('first\r\nsecond\r\n')
      await client.repositoryGitIgnore(existingFolder, { type: 'save', text: '' })
      expect(await client.repositoryGitIgnore(existingFolder, { type: 'read' })).toBeNull()
      await symlink(path.join(directory, 'keep-outside'), path.join(existingFolder, '.gitignore'))
      await writeFile(path.join(directory, 'keep-outside'), 'untouched')
      await expect(client.repositoryGitIgnore(existingFolder, { type: 'read' })).rejects.toThrow()
      await expect(client.repositoryGitIgnore(existingFolder, { type: 'save', text: 'bad' })).rejects.toThrow()
      expect(await readFile(path.join(directory, 'keep-outside'), 'utf8')).toBe('untouched')
      await unlink(path.join(existingFolder, '.gitignore'))
      expect(await client.snapshot()).toEqual(initialSelection)
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
      await expect(client.repositoryWorktrees(path.join(unselected, 'nested'))).resolves.toEqual([expect.objectContaining({ path: unselected, type: 'main' })])
      const worktreesBare = path.join(directory, 'worktrees-bare.git')
      execFileSync('git', ['init', '--bare', '-q', worktreesBare])
      await expect(client.repositoryWorktrees(worktreesBare)).resolves.toEqual([expect.objectContaining({ path: worktreesBare, type: 'main' })])
      await expect(client.repositoryWorktrees('relative')).rejects.toThrow()
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
      const binary = Buffer.alloc(2 * 1024 * 1024 + 9, Buffer.from([0, 255, 128, 10]))
      await writeFile(path.join(firstRepository, '-binary file'), binary)
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
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
      await writeFile(path.join(firstRepository, '-binary file'), 'changed')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      await client.repositoryResetIndex(firstRepository, ['-binary file'])
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('changed')
      await client.repositoryUpdateIndex(firstRepository, ['-binary file'], false)
      expect(execFileSync('git', ['-C', firstRepository, 'show', ':-binary file'], { encoding: 'utf8' })).toBe('changed')
      await client.repositoryUnstageAll(firstRepository)
      await client.repositoryCheckoutIndex(firstRepository, ['-binary file', 'missing'])
      expect(await readFile(path.join(firstRepository, '-binary file'))).toEqual(binary)
      await unlink(path.join(firstRepository, '-binary file'))
      await client.repositoryCheckoutHeadPaths(firstRepository, ['-binary file'])
      expect(await readFile(path.join(firstRepository, '-binary file'))).toEqual(binary)
      await client.repositoryCheckoutHeadPaths(firstRepository, [])
      await client.repositoryRemoveIndexAll(firstRepository)
      expect(execFileSync('git', ['-C', firstRepository, 'ls-files'], { encoding: 'utf8' })).toBe('')
      expect(await readFile(path.join(firstRepository, '-binary file'))).toEqual(binary)
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      await expect(client.repositoryCheckoutIndex(firstRepository, ['../outside'])).rejects.toThrow()
      const encodedCommit = await client.repositoryRendererCommit(firstRepository, 'HEAD')
      const trailerMessage = 'Summary\n\nReviewed-by: Old\n'
      execFileSync('git', ['-C', firstRepository, 'config', 'trailer.ifexists', 'replace'])
      const trailerArgs = ['interpret-trailers', '--no-divider', '--trailer', 'Reviewed-by=New']
      expect(await client.repositoryInterpretTrailers(firstRepository, trailerMessage, [{ token: 'Reviewed-by', value: 'New' }], false, false)).toBe(execFileSync('git', ['-C', firstRepository, ...trailerArgs], { encoding: 'utf8', input: trailerMessage }))
      expect(await client.repositoryInterpretTrailers(firstRepository, trailerMessage, [], true, false)).toBe(execFileSync('git', ['-C', firstRepository, 'interpret-trailers', '--parse'], { encoding: 'utf8', input: trailerMessage }))
      await expect(client.repositoryInterpretTrailers(firstRepository, '\0', [], false, false)).rejects.toThrow()
      const rootHead = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      expect(await client.repositoryRendererTagRefs(firstRepository)).toBe('')
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_create_annotated', name: 'typed-tag', target: 'HEAD' })
      expect(execFileSync('git', ['-C', firstRepository, 'cat-file', '-t', 'refs/tags/typed-tag'], { encoding: 'utf8' }).trim()).toBe('tag')
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'refs/tags/typed-tag^{}'], { encoding: 'utf8' }).trim()).toBe(rootHead)
      await expect(client.mutateRepositoryTag(firstRepository, { method: 'tag_create_annotated', name: 'typed-tag', target: 'HEAD' })).rejects.toThrow(/already exists/i)
      const rootTree = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD^{tree}'], { encoding: 'utf8' }).trim()
      const otherCommit = execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit-tree', rootTree, '-m', 'other'], { encoding: 'utf8' }).trim()
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_create_annotated', name: 'release,comma', target: otherCommit })
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'refs/tags/release,comma^{}'], { encoding: 'utf8' }).trim()).toBe(otherCommit)
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_create_annotated', name: 'typed-tag-two', target: 'HEAD' })
      const rootWithTags = Buffer.from((await client.repositoryRendererCommit(firstRepository, 'HEAD'))!, 'base64').toString('utf8')
      expect(rootWithTags).toContain('tag: typed-tag')
      expect(rootWithTags).toContain('tag: typed-tag-two')
      expect(Buffer.from((await client.repositoryRendererCommit(firstRepository, otherCommit))!, 'base64').toString('utf8')).toContain('tag: release,comma')
      expect(await client.repositoryRendererTagRefs(firstRepository)).toContain('refs/tags/release,comma^{}')
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_delete', name: 'release,comma' })
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_delete', name: 'typed-tag-two' })
      await client.mutateRepositoryTag(firstRepository, { method: 'tag_delete', name: 'typed-tag' })
      await expect(client.mutateRepositoryTag(firstRepository, { method: 'tag_delete', name: 'typed-tag' })).rejects.toThrow()
      expect(await client.repositoryRendererTagRefs(firstRepository)).toBe('')
      const rootIndex = await readFile(path.join(firstRepository, '.git', 'index'))
      const rootAmend = await client.repositorySelectedCommitPreview(firstRepository, { amend_head: rootHead })
      expect(rootAmend.review).toMatchObject({ staged_count: 1, files: [{ bytes: [...Buffer.from('-binary file')] }] })
      expect(await readFile(path.join(firstRepository, '.git', 'index'))).toEqual(rootIndex)
      execFileSync('git', ['-C', firstRepository, 'tag', 'lightweight'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'tag', '-a', 'annotated', '-m', 'tag message'])
      expect(await client.repositoryRendererTagRefs(firstRepository)).toBe(execFileSync('git', ['-C', firstRepository, 'show-ref', '--tags', '-d'], { encoding: 'utf8' }))
      const changedArgs = ['log', 'HEAD', '-C', '-M', '-m', '-1', '--no-show-signature', '--first-parent', '--raw', '--format=format:', '--numstat', '-z', '--']
      expect(await client.repositoryRendererCommitChanges(firstRepository, 'HEAD')).toBe(execFileSync('git', ['-C', firstRepository, ...changedArgs], { encoding: 'utf8' }))
      await expect(client.repositoryRendererCommitChanges(firstRepository, 'missing')).rejects.toThrow()
      const commitDiffArgs = ['log', 'HEAD', '-m', '-1', '--first-parent', '--patch-with-raw', '--format=', '-z', '--no-color', '--', ':(literal)-binary file']
      expect(Buffer.from(await client.repositoryRendererCommitDiff(firstRepository, 'HEAD', '-binary file', undefined, false) as Uint8Array)).toEqual(execFileSync('git', ['-C', firstRepository, ...commitDiffArgs]))
      await expect(client.repositoryRendererCommitDiff(firstRepository, 'HEAD', '../outside', undefined, false)).rejects.toThrow()
      execFileSync('git', ['-C', firstRepository, 'tag', '-d', 'lightweight', 'annotated'])
      const initialBranch = execFileSync('git', ['-C', firstRepository, 'symbolic-ref', '--short', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'checkout', '-qb', 'recent-old'])
      execFileSync('git', ['-C', firstRepository, 'branch', '-m', 'recent-new'])
      const branchLog = execFileSync('git', ['-C', firstRepository, 'log', '-g', '--no-abbrev-commit', '--pretty=oneline', 'HEAD', '-n', '2500', '--'], { encoding: 'utf8' })
      expect(await client.repositoryRendererReflog(firstRepository, 'branches')).toBe(branchLog)
      const cutoff = new Date('2000-01-01T00:00:00.000Z')
      const checkoutArgs = ['reflog', '--date=iso', `--after="${cutoff.toISOString()}"`, '--pretty=%H %gd %gs', '--grep-reflog=checkout: moving from .* to .*$', '--']
      expect(await client.repositoryBranchCheckouts(firstRepository, cutoff)).toBe(execFileSync('git', ['-C', firstRepository, ...checkoutArgs], { encoding: 'utf8' }))
      expect(await client.repositoryBranchCheckouts(firstRepository, new Date('2035-01-01T00:00:00.000Z'))).toBe('')
      await expect(client.repositoryBranchCheckouts(firstRepository, new Date('2999-01-01T00:00:00.000Z'))).rejects.toThrow()
      await writeFile(path.join(firstRepository, 'stash-fixture'), 'stash content\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', 'stash-fixture'])
      execFileSync('git', ['-C', firstRepository, 'stash', 'push', '-qm', '!!GitHub_Desktop<recent-new>'])
      const stashLog = execFileSync('git', ['-C', firstRepository, 'log', '-g', '-z', '--format=%gD%x00%H%x00%gs%x00%T%x00%P', 'refs/stash', '--'], { encoding: 'utf8' })
      expect(await client.repositoryRendererReflog(firstRepository, 'stash')).toBe(stashLog)
      execFileSync('git', ['-C', firstRepository, 'stash', 'drop', '-q'])
      execFileSync('git', ['-C', firstRepository, 'update-ref', 'refs/remotes/origin/main', initialBranch])
      await writeFile(path.join(firstRepository, 'history-fixture'), 'second commit\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', 'history-fixture'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '-qm', 'second'])
      const historyArgs = ['log', '--date=raw', '--max-count=1', '--skip=0', '-z', '--format=%H%x00%h%x00%s%x00%b%x00%an <%ae> %ad%x00%cn <%ce> %cd%x00%P%x00%(trailers:unfold,only)%x00%D', '--no-show-signature', '--no-color', '--not', '--remotes', '--not', '--end-of-options', 'HEAD', '--']
      const expectedHistory = execFileSync('git', ['-C', firstRepository, ...historyArgs])
      const history = await client.repositoryRendererHistory(firstRepository, 'HEAD', 1, 0, false, true)
      expect(Buffer.from(history!, 'base64')).toEqual(expectedHistory)
      const parentArgs = ['log', '--date=raw', '--max-count=2', '-z', '--format=%H%x00%h%x00%s%x00%b%x00%an <%ae> %ad%x00%cn <%ce> %cd%x00%P%x00%(trailers:unfold,only)%x00%D', '--no-show-signature', '--no-color', '--first-parent', '--end-of-options', 'HEAD', '--']
      expect(Buffer.from((await client.repositoryRendererHistory(firstRepository, 'HEAD', 2, undefined, true, false))!, 'base64')).toEqual(execFileSync('git', ['-C', firstRepository, ...parentArgs]))
      const rangeArgs = [...parentArgs]
      rangeArgs[rangeArgs.length - 2] = 'HEAD~1..HEAD'
      expect(Buffer.from((await client.repositoryRendererHistory(firstRepository, 'HEAD~1..HEAD', 2, undefined, true, false))!, 'base64')).toEqual(execFileSync('git', ['-C', firstRepository, ...rangeArgs]))
      await expect(client.repositoryRendererHistory(firstRepository, 'missing', 1, 0, false, false)).resolves.toBeNull()
      execFileSync('git', ['-C', firstRepository, 'update-ref', '-d', 'refs/remotes/origin/main'])
      execFileSync('git', ['-C', firstRepository, 'checkout', '-q', initialBranch])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'recent-new'])
      const linked = path.join(directory, 'linked\nworktree')
      execFileSync('git', ['-C', firstRepository, 'worktree', 'add', '--detach', '-q', linked, 'HEAD'])
      const linkedGitDir = execFileSync('git', ['-C', linked, 'rev-parse', '--absolute-git-dir'], { encoding: 'utf8' }).trim()
      const protectedLinked = await client.repositoryProtectedResources(linked)
      expect(protectedLinked.paths).toEqual(expect.arrayContaining([linked, linkedGitDir, path.join(firstRepository, '.git')]))
      const shared = path.join(directory, 'shared-clone')
      execFileSync('git', ['clone', '-q', '--shared', firstRepository, shared])
      const protectedShared = await client.repositoryProtectedResources(shared)
      expect(protectedShared.objectDirectories).toContain(path.join(firstRepository, '.git', 'objects'))
      await expect(client.repositoryProtectedResources('relative')).rejects.toThrow()
      execFileSync('git', ['-C', firstRepository, 'worktree', 'lock', '--reason', 'test reason', linked])
      const worktrees = await client.repositoryWorktrees(firstRepository)
      expect(worktrees).toEqual([
        expect.objectContaining({ path: firstRepository, type: 'main', isDetached: false }),
        expect.objectContaining({ path: linked, type: 'linked', isDetached: true, isLocked: true, isPrunable: false }),
      ])
      execFileSync('git', ['-C', firstRepository, 'worktree', 'unlock', linked])
      const temporarilyMoved = path.join(directory, 'temporarily-moved')
      await rename(linked, temporarilyMoved)
      expect(await client.repositoryWorktrees(firstRepository)).toEqual(expect.arrayContaining([expect.objectContaining({ path: linked, isPrunable: true })]))
      expect(await client.repositoryWorktrees(linkedGitDir, true)).toEqual(expect.arrayContaining([
        expect.objectContaining({ path: firstRepository, type: 'main' }),
        expect.objectContaining({ path: linked, isPrunable: true }),
      ]))
      await expect(client.repositoryWorktrees('relative', true)).rejects.toThrow()
      await rename(temporarilyMoved, linked)
      execFileSync('git', ['-C', firstRepository, 'worktree', 'remove', linked])
      await expect(client.repositoryWorktrees(linkedGitDir, true)).rejects.toThrow()
      expect(Buffer.from(encodedCommit!, 'base64').toString('utf8')).toContain('initial')
      await expect(client.repositoryRendererCommit(firstRepository, 'missing')).resolves.toBeNull()
      const blob = await client.repositoryBlobPrefix(firstRepository, 'HEAD', '-binary file', 1024)
      expect(Buffer.from(blob!, 'base64')).toEqual(binary.subarray(0, 1024))
      const chunks: Buffer[] = []
      let blobOid: string | null = null
      for (let offset = 0; offset < binary.length; offset += 1024 * 1024) {
        const chunk = await client.repositoryBlobChunk(firstRepository, 'HEAD', '-binary file', offset, 1024 * 1024, blobOid)
        expect(chunk).not.toBeNull()
        expect(chunk!.size).toBe(binary.length)
        blobOid = chunk!.oid
        chunks.push(Buffer.from(chunk!.data, 'base64'))
      }
      expect(Buffer.concat(chunks)).toEqual(binary)
      await expect(client.repositoryBlobChunk(firstRepository, 'HEAD', '-binary file', 0, 1024, '0'.repeat(40))).rejects.toThrow()
      expect(await client.repositoryBlobChunk(firstRepository, 'HEAD', 'missing', 0, 1024, null)).toBeNull()
      const indexBlob = await client.repositoryBlobChunk(firstRepository, ':0', '-binary file', 0, 1024, null)
      await writeFile(path.join(firstRepository, '-binary file'), 'new index content')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      await expect(client.repositoryBlobChunk(firstRepository, ':0', '-binary file', 1024, 1024, indexBlob!.oid)).rejects.toThrow()
      await writeFile(path.join(firstRepository, '-binary file'), binary)
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      expect(await client.repositoryBlobPrefix(firstRepository, 'HEAD', '-binary file', 0)).toBe('')
      expect(await client.repositoryBlobPrefix(firstRepository, 'HEAD', 'missing', 1024)).toBeNull()
      expect(Buffer.from((await client.repositoryBlobPrefix(firstRepository, '', '-binary file', 4))!, 'base64')).toEqual(binary.subarray(0, 4))
      execFileSync('git', ['-C', firstRepository, 'update-ref', 'refs/remotes/--remote/base', 'HEAD'])
      const branchHead = execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()
      const stashFile = path.join(firstRepository, 'saved-untracked.txt')
      await writeFile(stashFile, 'saved untracked content\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', 'saved-untracked.txt'])
      expect(await client.saveDesktopStash(firstRepository, '--remote/base')).toBe(true)
      const savedStash = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'stash@{0}'], { encoding: 'utf8' }).trim()
      expect(execFileSync('git', ['-C', firstRepository, 'stash', 'list'], { encoding: 'utf8' })).toContain('!!GitHub_Desktop<--remote/base>')
      expect(await client.repositoryStashChanges(firstRepository, savedStash)).toBe(execFileSync('git', ['-C', firstRepository, 'stash', 'show', savedStash, '--raw', '--numstat', '-z', '--format=format:', '--no-show-signature', '--'], { encoding: 'utf8' }))
      await expect(client.repositoryStashChanges(firstRepository, 'HEAD')).rejects.toMatchObject({ code: 'invalid_request' })
      await client.mutateDesktopStash(firstRepository, savedStash, 'pop')
      expect(await readFile(stashFile, 'utf8')).toBe('saved untracked content\n')
      expect(execFileSync('git', ['-C', firstRepository, 'diff', '--cached', '--name-only'], { encoding: 'utf8' })).toContain('saved-untracked.txt')
      execFileSync('git', ['-C', firstRepository, 'reset', '-q', 'HEAD', '--', 'saved-untracked.txt'])
      await unlink(stashFile)
      const beforeConflict = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'stash-conflict-test'])
      await writeFile(path.join(firstRepository, '-binary file'), 'saved side\n')
      expect(await client.saveDesktopStash(firstRepository, 'stash-conflict-test')).toBe(true)
      const conflictStash = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'stash@{0}'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '-binary file'), 'committed side\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'stash conflict'])
      await client.mutateDesktopStash(firstRepository, conflictStash, 'pop')
      expect(execFileSync('git', ['-C', firstRepository, 'ls-files', '--unmerged'], { encoding: 'utf8' })).not.toBe('')
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'stash@{0}'], { encoding: 'utf8' }).trim()).toBe(conflictStash)
      execFileSync('git', ['-C', firstRepository, 'reset', '--hard', '-q', beforeConflict])
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'stash-conflict-test'])
      execFileSync('git', ['-C', firstRepository, 'stash', 'drop', '-q'])
      await client.createRepositoryBranch(firstRepository, 'new-from-remote', '--remote/base', true)
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'new-from-remote'], { encoding: 'utf8' }).trim()).toBe(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim())
      expect(execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(branchHead)
      const switchTip = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '-binary file'), 'carry local changes\n')
      const switchReview = await client.reviewBranchCheckout(firstRepository, 'refs/heads/new-from-remote', switchTip)
      expect(switchReview.dirty).toBe(true)
      await client.checkoutReviewedBranch(firstRepository, 'refs/heads/new-from-remote', switchReview.basis, 'MoveToNewBranch', false)
      expect(execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()).toBe('refs/heads/new-from-remote')
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('carry local changes\n')
      const returnReview = await client.reviewBranchCheckout(firstRepository, branchHead, switchTip)
      await client.checkoutReviewedBranch(firstRepository, branchHead, returnReview.basis, 'MoveToNewBranch', false)
      await client.checkoutDirectBranch(firstRepository, 'refs/heads/new-from-remote', switchTip, false)
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('carry local changes\n')
      await client.checkoutDirectBranch(firstRepository, branchHead, switchTip, false)
      await client.checkoutDetachedCommit(firstRepository, rootHead, false)
      expect(spawnSync('git', ['-C', firstRepository, 'symbolic-ref', '--quiet', 'HEAD']).status).toBe(1)
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(rootHead)
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('carry local changes\n')
      await client.checkoutDirectBranch(firstRepository, branchHead, switchTip, false)
      await writeFile(path.join(firstRepository, '-binary file'), binary)
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'checkout-conflict-test'])
      await writeFile(path.join(firstRepository, '-binary file'), 'target change\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'checkout target'])
      const conflictTip = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      await writeFile(path.join(firstRepository, '-binary file'), 'local change\n')
      await expect(client.checkoutDirectBranch(firstRepository, 'refs/heads/checkout-conflict-test', conflictTip, false)).rejects.toThrow()
      await expect(client.checkoutDetachedCommit(firstRepository, conflictTip, false)).rejects.toThrow()
      expect(execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(branchHead)
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('local change\n')
      expect(execFileSync('git', ['-C', firstRepository, 'stash', 'list'], { encoding: 'utf8' })).toBe('')
      await writeFile(path.join(firstRepository, '-binary file'), binary)
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'checkout-conflict-test'])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'revert-test'])
      await writeFile(path.join(firstRepository, '-binary file'), 'selected\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'selected change'])
      const revertTarget = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await client.revertReviewedCommit(firstRepository, revertTarget, { head: revertTarget, branch: 'refs/heads/revert-test', merge: false })
      expect(await readFile(path.join(firstRepository, '-binary file'))).toEqual(binary)
      await writeFile(path.join(firstRepository, '-binary file'), 'selected again\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'selected again'])
      const conflictRevert = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '-binary file'), 'newer change\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'newer change'])
      const revertHead = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await expect(client.revertReviewedCommit(firstRepository, conflictRevert, { head: revertHead, branch: 'refs/heads/revert-test', merge: false })).rejects.toMatchObject({ code: 'revert_conflict' })
      expect(execFileSync('git', ['-C', firstRepository, 'ls-files', '--unmerged'], { encoding: 'utf8' })).not.toBe('')
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'REVERT_HEAD'], { encoding: 'utf8' }).trim()).toBe(conflictRevert)
      execFileSync('git', ['-C', firstRepository, 'revert', '--abort'])
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'revert-test'])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'pick-source'])
      await writeFile(path.join(firstRepository, '-binary file'), 'pick source\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'pick source'])
      const pickTarget = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'pick-destination'])
      await writeFile(path.join(firstRepository, '-binary file'), 'pick destination\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'pick destination'])
      const pickDestination = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await expect(client.startRepositoryCherryPick(firstRepository, [pickTarget], { head: '0'.repeat(40), branch: 'refs/heads/pick-destination' })).rejects.toThrow()
      await expect(client.startRepositoryCherryPick(firstRepository, [pickTarget], { head: pickDestination, branch: 'refs/heads/pick-destination' })).resolves.toBe('conflict')
      await expect(client.continueRepositoryCherryPick(firstRepository, pickTarget, false)).rejects.toThrow()
      await writeFile(path.join(firstRepository, '-binary file'), 'resolved pick\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      await expect(client.continueRepositoryCherryPick(firstRepository, '0'.repeat(40), false)).rejects.toThrow()
      await expect(client.continueRepositoryCherryPick(firstRepository, pickTarget, false)).resolves.toBe('complete')
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('resolved pick\n')
      expect(spawnSync('git', ['-C', firstRepository, 'rev-parse', '--verify', '--quiet', 'CHERRY_PICK_HEAD']).status).toBe(1)
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'pick-destination', 'pick-source'])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'rebase-source'])
      await writeFile(path.join(firstRepository, '-binary file'), 'source rebase\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'rebase source'])
      const rebaseTarget = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'rebase-base'])
      await writeFile(path.join(firstRepository, '-binary file'), 'base rebase\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'rebase base'])
      const rebaseBase = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', 'rebase-source'])
      const rebaseRequest = { base_ref: 'refs/heads/rebase-base', base_head: rebaseBase, target_ref: 'refs/heads/rebase-source', target_head: rebaseTarget, current_head: rebaseTarget, current_branch: 'refs/heads/rebase-source' }
      await expect(client.startRepositoryRebase(firstRepository, { ...rebaseRequest, target_head: '0'.repeat(40) })).rejects.toThrow()
      await expect(client.startRepositoryRebase(firstRepository, rebaseRequest)).resolves.toBe('conflict')
      await expect(client.continueRepositoryRebase(firstRepository, rebaseTarget, false, false)).rejects.toThrow()
      await writeFile(path.join(firstRepository, '-binary file'), 'resolved rebase\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      await expect(client.continueRepositoryRebase(firstRepository, '0'.repeat(40), false, false)).rejects.toThrow()
      await expect(client.continueRepositoryRebase(firstRepository, rebaseTarget, false, false)).resolves.toBe('complete')
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('resolved rebase\n')
      expect(await client.repositoryRebaseState(firstRepository)).toBeNull()
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'rebase-source', 'rebase-base'])
      execFileSync('git', ['-C', firstRepository, 'switch', '-qc', 'reset-test'])
      const resetTarget = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '-binary file'), 'reset commit\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', '-binary file'])
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'reset commit'])
      const resetHead = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '-binary file'), 'local reset content\n')
      const resetRequest = { target: resetTarget, head: resetHead, branch: 'refs/heads/reset-test', undo: true }
      await expect(client.mixedResetRepository(firstRepository, { ...resetRequest, target: resetHead })).rejects.toThrow()
      await client.mixedResetRepository(firstRepository, resetRequest)
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(resetTarget)
      expect(await readFile(path.join(firstRepository, '-binary file'), 'utf8')).toBe('local reset content\n')
      await writeFile(path.join(firstRepository, '-binary file'), binary)
      execFileSync('git', ['-C', firstRepository, 'switch', '-q', branchHead.replace(/^refs\/heads\//, '')])
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'reset-test'])
      expect(spawnSync('git', ['-C', firstRepository, 'config', '--get', 'branch.new-from-remote.remote']).status).toBe(1)
      execFileSync('git', ['-C', firstRepository, 'remote', 'add', 'typed-test', 'https://example.invalid/repo'])
      execFileSync('git', ['-C', firstRepository, 'update-ref', 'refs/remotes/typed-test/main', 'HEAD'])
      await client.createRepositoryBranch(firstRepository, 'new-tracking', 'typed-test/main', false)
      expect(execFileSync('git', ['-C', firstRepository, 'config', '--get', 'branch.new-tracking.remote'], { encoding: 'utf8' }).trim()).toBe('typed-test')
      expect(execFileSync('git', ['-C', firstRepository, 'config', '--get', 'branch.new-tracking.merge'], { encoding: 'utf8' }).trim()).toBe('refs/heads/main')
      await writeFile(path.join(firstRepository, '-binary file'), 'saved branch changes')
      execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'stash', 'push', '-m', '!!GitHub_Desktop<new-tracking>', '--', '-binary file'])
      const savedTree = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'stash@{0}^{tree}'], { encoding: 'utf8' }).trim()
      const trackingTip = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'new-tracking'], { encoding: 'utf8' }).trim()
      await expect(client.renameRepositoryBranch(firstRepository, 'new-tracking', 'renamed-tracking', '0'.repeat(40))).rejects.toMatchObject({ code: 'branch_changed' })
      await client.renameRepositoryBranch(firstRepository, 'new-tracking', 'renamed-tracking', trackingTip)
      expect(execFileSync('git', ['-C', firstRepository, 'rev-parse', 'stash@{0}^{tree}'], { encoding: 'utf8' }).trim()).toBe(savedTree)
      expect(execFileSync('git', ['-C', firstRepository, 'stash', 'list'], { encoding: 'utf8' })).toContain('!!GitHub_Desktop<renamed-tracking>')
      expect(execFileSync('git', ['-C', firstRepository, 'config', '--get', 'branch.renamed-tracking.remote'], { encoding: 'utf8' }).trim()).toBe('typed-test')
      execFileSync('git', ['-C', firstRepository, 'stash', 'drop', 'stash@{0}'])
      const branchTip = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'new-from-remote'], { encoding: 'utf8' }).trim()
      await expect(client.deleteRepositoryBranch(firstRepository, 'new-from-remote', '0'.repeat(40))).rejects.toMatchObject({ code: 'branch_changed' })
      await client.deleteRepositoryBranch(firstRepository, 'new-from-remote', branchTip)
      expect(spawnSync('git', ['-C', firstRepository, 'show-ref', '--verify', '--quiet', 'refs/heads/new-from-remote']).status).toBe(1)
      expect(execFileSync('git', ['-C', firstRepository, 'for-each-ref', '--format=%(objectname)', 'refs/desktop/deleted-branches/'], { encoding: 'utf8' })).toContain(branchTip)
      execFileSync('git', ['-C', firstRepository, 'branch', '-D', 'renamed-tracking'])
      execFileSync('git', ['-C', firstRepository, 'remote', 'remove', 'typed-test'])
      expect(Buffer.from((await client.repositoryBlobPrefix(firstRepository, '--remote/base', '-binary file', 4))!, 'base64')).toEqual(binary.subarray(0, 4))
      expect(Buffer.from((await client.repositoryBlobPrefix(firstRepository, ':0', '-binary file', 4))!, 'base64')).toEqual(binary.subarray(0, 4))
      await expect(client.repositoryBlobPrefix(firstRepository, 'missing', '-binary file', 4)).rejects.toThrow()
      await expect(client.repositoryBlobPrefix(firstRepository, 'HEAD', '../outside', 4)).rejects.toThrow()
      const firstSnapshot = (await bounded(client.snapshot())) as {
        root_id: string
      }
      const headForMerge = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      expect(await client.repositorySubmoduleStatus(firstRepository)).toBe(execFileSync('git', ['-C', firstRepository, 'submodule', 'status', '--'], { encoding: 'utf8' }))
      await expect(client.repositoryResetSubmodules(firstRepository, ['../outside'])).rejects.toThrow()
      const moduleSource = path.join(directory, 'module-source')
      execFileSync('git', ['init', '-q', moduleSource])
      await writeFile(path.join(moduleSource, 'tracked'), 'original\n')
      execFileSync('git', ['-C', moduleSource, 'add', 'tracked'])
      execFileSync('git', ['-C', moduleSource, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'initial'])
      execFileSync('git', ['-C', firstRepository, '-c', 'protocol.file.allow=always', 'submodule', 'add', '-q', moduleSource, 'module with space'])
      const submoduleFile = path.join(firstRepository, 'module with space', 'tracked')
      await writeFile(submoduleFile, 'changed\n')
      await client.repositoryResetSubmodules(firstRepository, ['module with space'])
      expect(await readFile(submoduleFile, 'utf8')).toBe('original\n')
      execFileSync('git', ['-C', firstRepository, 'submodule', 'deinit', '-f', '--', 'module with space'])
      await expect(readFile(submoduleFile)).rejects.toMatchObject({ code: 'ENOENT' })
      await client.repositoryUpdateSubmodules(firstRepository, true)
      expect(await readFile(submoduleFile, 'utf8')).toBe('original\n')
      execFileSync('git', ['-C', firstRepository, 'rm', '-f', '-q', '--', 'module with space'])
      execFileSync('git', ['-C', firstRepository, 'rm', '--cached', '-f', '-q', '--', '.gitmodules'])
      await unlink(path.join(firstRepository, '.gitmodules'))
      expect(await client.repositoryMergeBase(firstRepository, 'HEAD', headForMerge)).toBe(headForMerge)
      expect(await client.repositoryMergeBase(firstRepository, 'missing-ref', 'HEAD')).toBeNull()
      expect(await client.repositoryMergeTreePreview(firstRepository, headForMerge, headForMerge)).toEqual({ kind: 'clean' })
      await writeFile(path.join(firstRepository, 'untracked'), 'new file')
      const indexBeforeReview = await readFile(path.join(firstRepository, '.git', 'index'))
      const selectedReview = await client.repositorySelectedCommitPreview(firstRepository, { paths: ['untracked'] })
      const emptyReview = await client.repositorySelectedCommitPreview(firstRepository, { allow_empty: true })
      expect(emptyReview.review).toMatchObject({ staged_count: 0, files: [] })
      expect(selectedReview.basis).toMatch(/^[a-f0-9]{64}$/)
      expect(selectedReview.review).toMatchObject({ staged_count: 1, files: [{ bytes: [...Buffer.from('untracked')] }], large_files: [] })
      expect(await client.repositorySelectedCommitPreview(firstRepository, { paths: ['untracked'] })).toEqual(selectedReview)
      const checkpoint = await client.prepareSelectedCommit(firstRepository, { paths: ['untracked'] }, selectedReview.basis)
      expect(await client.listCommitCheckpoints(firstRepository)).toEqual([expect.objectContaining({ id: checkpoint, state: 'prepared' })])
      expect(await client.inspectCommitCheckpoint(firstRepository, checkpoint)).toMatchObject({ recorded_files_verified: true, index_coverage_verified: true, head_matches: true, branch_matches: true, index_matches: true, operation_matches: true })
      expect(await client.revalidateCommitCheckpoint(firstRepository, checkpoint)).toEqual({ basis: selectedReview.basis, matches: true })
      await expect(client.recoverPublishedCommitCheckpoint(firstRepository, checkpoint)).rejects.toThrow('does not have a recoverable published commit')
      const savedIndex = path.join(firstRepository, '.git', 'desktop-commit-checkpoints', checkpoint, 'index.selected')
      const originalSelectedIndex = await readFile(savedIndex)
      await writeFile(savedIndex, 'damaged')
      await expect(client.inspectCommitCheckpoint(firstRepository, checkpoint)).rejects.toThrow()
      await expect(client.revalidateCommitCheckpoint(firstRepository, checkpoint)).rejects.toThrow()
      await writeFile(savedIndex, originalSelectedIndex)
      await writeFile(path.join(firstRepository, 'untracked'), 'newer file')
      expect(await client.revalidateCommitCheckpoint(firstRepository, checkpoint)).toEqual({ basis: selectedReview.basis, matches: false })
      await client.forgetCommitCheckpoint(firstRepository, checkpoint)
      expect(await client.listCommitCheckpoints(firstRepository)).toEqual([])
      expect(await readFile(path.join(firstRepository, '.git', 'index'))).toEqual(indexBeforeReview)
      await writeFile(path.join(firstRepository, 'untracked'), 'newer file')
      expect((await client.repositorySelectedCommitPreview(firstRepository, { paths: ['untracked'] })).basis).not.toBe(selectedReview.basis)
      await expect(client.prepareSelectedCommit(firstRepository, { paths: ['untracked'] }, selectedReview.basis)).rejects.toThrow()
      await writeFile(path.join(firstRepository, 'untracked'), 'new file')
      const rendererDiff = await client.repositoryRendererWorkingDiff(firstRepository, 'untracked', 'empty', false, false)
      expect(Buffer.from(rendererDiff.stdout, 'base64').toString('utf8')).toContain('+new file')
      await expect(client.repositoryRendererWorkingDiff(firstRepository, '../outside', 'empty', false, false)).rejects.toThrow()
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
      const partialReview = await client.repositorySelectedCommitPreview(firstRepository, { partial_selections: [{ path: file.path, basis: preview.basis, lines: [0] }] })
      expect(partialReview.basis).not.toBe(selectedReview.basis)
      expect(await readFile(path.join(firstRepository, '.git', 'index'))).toEqual(indexBeforeReview)
      await writeFile(path.join(firstRepository, 'partial-stage'), 'first\nsecond\n')
      const stagePreview = await client.repositoryWorkingDiff(firstRepository, 'partial-stage')
      await client.repositoryStageSelectedLines(firstRepository, 'partial-stage', stagePreview.basis, [stagePreview.hunks[0].lines[0].id])
      expect(execFileSync('git', ['-C', firstRepository, 'show', ':partial-stage'], { encoding: 'utf8' })).toBe('first\n')
      execFileSync('git', ['-C', firstRepository, 'rm', '--cached', '-f', '-q', '--', 'partial-stage'])
      await unlink(path.join(firstRepository, 'partial-stage'))
      const working = await client.repositoryWorkingDiff(firstRepository, file.path)
      const selectedLines = getReviewedDiffLines(file.path, file.selection, displayed, working, 'stale')
      const undoId = await client.repositoryDiscardSelection(firstRepository, file.path, { basis: working.basis, lines: selectedLines })
      expect(await client.repositoryDiscardHistory(firstRepository)).toEqual(expect.arrayContaining([expect.objectContaining({ id: undoId, state: 'applied', files: ['untracked'] })]))
      await expect(readFile(path.join(firstRepository, 'untracked'))).rejects.toMatchObject({ code: 'ENOENT' })
      await client.undoRepositoryDiscard(firstRepository, undoId)
      expect(await client.repositoryDiscardHistory(firstRepository)).toEqual(expect.arrayContaining([expect.objectContaining({ id: undoId, state: 'undone' })]))
      await client.forgetRepositoryDiscard(firstRepository, undoId)
      expect(await client.repositoryDiscardHistory(firstRepository)).not.toEqual(expect.arrayContaining([expect.objectContaining({ id: undoId })]))
      expect(await readFile(path.join(firstRepository, 'untracked'), 'utf8')).toBe('new file')
      await writeFile(path.join(firstRepository, 'untracked'), 'newer content')
      await expect(client.repositoryDiscardSelection(firstRepository, file.path, { basis: working.basis, lines: selectedLines })).rejects.toThrow()
      expect(await readFile(path.join(firstRepository, 'untracked'), 'utf8')).toBe('newer content')
      await writeFile(path.join(firstRepository, 'untracked'), 'new file')
      await writeFile(path.join(firstRepository, 'another-untracked'), 'another file')
      const batchReview = await client.repositoryDiscardBatchPreview(firstRepository, ['untracked', 'another-untracked'])
      expect(batchReview.files).toEqual(['untracked', 'another-untracked'])
      const batchUndo = await client.repositoryDiscardBatch(firstRepository, ['untracked', 'another-untracked'], batchReview.basis)
      await expect(readFile(path.join(firstRepository, 'untracked'))).rejects.toMatchObject({ code: 'ENOENT' })
      await expect(readFile(path.join(firstRepository, 'another-untracked'))).rejects.toMatchObject({ code: 'ENOENT' })
      await client.undoRepositoryDiscard(firstRepository, batchUndo)
      expect(await readFile(path.join(firstRepository, 'untracked'), 'utf8')).toBe('new file')
      expect(await readFile(path.join(firstRepository, 'another-untracked'), 'utf8')).toBe('another file')
      await unlink(path.join(firstRepository, 'another-untracked'))
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
      const largeStatusDirectory = path.join(firstRepository, 'large-status')
      await mkdir(largeStatusDirectory)
      const padding = 'x'.repeat(228)
      for (let batch = 0; batch < 92; batch++) {
        await Promise.all(Array.from({ length: 100 }, (_, offset) =>
          writeFile(path.join(largeStatusDirectory, `${String(batch * 100 + offset).padStart(5, '0')}-${padding}`), '')
        ))
      }
      const largeStatus = await client.detailedRepositoryStatus(firstRepository)
      expect(largeStatus.filter(record => record.kind === 'entry' && Buffer.from(record.path.bytes).toString().startsWith('large-status/'))).toHaveLength(9200)
      await rm(largeStatusDirectory, { recursive: true, force: true })
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
      const mergeRequest = { source_ref: 'refs/heads/conflicting', source_head: execFileSync('git', ['-C', firstRepository, 'rev-parse', 'refs/heads/conflicting'], { encoding: 'utf8' }).trim(), current_ref: execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim(), current_head: execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), squash: false, no_verify: false }
      await expect(client.startRepositoryMerge(firstRepository, { ...mergeRequest, source_head: '0'.repeat(40) })).rejects.toThrow()
      await expect(client.startRepositoryMerge(firstRepository, mergeRequest)).resolves.toBe('conflict')
      const conflict = await client.repositoryConflictDetails(
        firstRepository,
        'merge'
      )
      expect(conflict.check).toContain('conflict.txt:')
      expect(conflict.check).toContain('leftover conflict marker')
      expect(conflict.attributes).toContain('conflict.txt\0merge\0')
      expect(conflict.attributes).toContain('binary.dat\0merge\0binary\0')
      await expect(client.repositoryResolveManualConflict(firstRepository, 'conflict.txt', 'ours', { ours: true, theirs: false })).rejects.toMatchObject({ code: 'conflict_changed' })
      await client.repositoryResolveManualConflict(firstRepository, 'conflict.txt', 'ours', { ours: true, theirs: true })
      expect(await readFile(path.join(firstRepository, 'conflict.txt'), 'utf8')).toBe('main\n')
      expect(execFileSync('git', ['-C', firstRepository, 'ls-files', '--unmerged', '--', 'conflict.txt'], { encoding: 'utf8' })).toBe('')
      await expect(client.abortRepositoryGitOperation(firstRepository, 'rebase')).rejects.toMatchObject({ retryable: false })
      await client.abortRepositoryGitOperation(firstRepository, 'merge')
      expect(await readFile(path.join(firstRepository, '.git', 'MERGE_HEAD'), 'utf8').catch(() => null)).toBeNull()
      expect(await readFile(path.join(firstRepository, 'conflict.txt'), 'utf8')).toBe('main\n')
      expect(() => execFileSync('git', ['-C', firstRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'merge', 'conflicting'], { stdio: 'pipe' })).toThrow()
      await writeFile(path.join(firstRepository, 'conflict.txt'), 'resolved\n')
      execFileSync('git', ['-C', firstRepository, 'add', '--', 'conflict.txt', 'binary.dat'])
      const mergeOutput = await client.finishRepositoryMergeCommit(firstRepository)
      expect(mergeOutput).toContain(']')
      expect(execFileSync('git', ['-C', firstRepository, 'rev-list', '--parents', '-n', '1', 'HEAD'], { encoding: 'utf8' }).trim().split(' ')).toHaveLength(3)
      const squashHead = execFileSync('git', ['-C', firstRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      const squashBranch = execFileSync('git', ['-C', firstRepository, 'symbolic-ref', 'HEAD'], { encoding: 'utf8' }).trim()
      await writeFile(path.join(firstRepository, '.git', 'SQUASH_MSG'), 'squash\n')
      await writeFile(path.join(firstRepository, 'conflict.txt'), 'changed\n')
      await expect(client.abortRepositorySquashMerge(firstRepository, squashHead, 'refs/heads/wrong')).rejects.toMatchObject({ retryable: false })
      expect(await readFile(path.join(firstRepository, 'conflict.txt'), 'utf8')).toBe('changed\n')
      await client.abortRepositorySquashMerge(firstRepository, squashHead, squashBranch)
      expect(await readFile(path.join(firstRepository, 'conflict.txt'), 'utf8')).toBe('resolved\n')
      expect(await readFile(path.join(firstRepository, '.git', 'SQUASH_MSG'), 'utf8').catch(() => null)).toBeNull()
      await bounded(
        client.request('init_repository', { path: secondRepository })
      )
      await expect(client.repositoryDiscardHistory(firstRepository)).rejects.toMatchObject({ code: 'repository_changed' })
      await expect(client.undoRepositoryDiscard(firstRepository, undoId)).rejects.toMatchObject({ code: 'repository_changed', retryable: false })
      await expect(client.forgetRepositoryDiscard(firstRepository, undoId)).rejects.toMatchObject({ code: 'repository_changed', retryable: false })
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

      const renameRepository = path.join(directory, 'partial-rename')
      await client.request('init_repository', { path: renameRepository })
      await writeFile(path.join(renameRepository, 'old'), 'before\n')
      execFileSync('git', ['-C', renameRepository, 'add', 'old'])
      execFileSync('git', ['-C', renameRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'initial'])
      execFileSync('git', ['-C', renameRepository, 'mv', 'old', 'new'])
      await writeFile(path.join(renameRepository, 'new'), 'after\n')
      await writeFile(path.join(renameRepository, 'old'), 'replacement\nextra\n')
      await client.selectBrowserRepository(renameRepository)
      const renamePreview = await client.copilotCommitDiffForRepository(renameRepository, 'new', {}, 'old')
      execFileSync('git', ['-C', renameRepository, 'reset', '--quiet', 'HEAD', '--', '.'])
      await client.repositoryStageRenamedLines(renameRepository, 'new', 'old', renamePreview.basis, renamePreview.hunks.flatMap(hunk => hunk.lines.filter(line => line.kind !== 'context').map(line => line.id)))
      expect(execFileSync('git', ['-C', renameRepository, 'show', ':new'], { encoding: 'utf8' })).toBe('after\n')
      expect(spawnSync('git', ['-C', renameRepository, 'show', ':old']).status).not.toBe(0)
      expect(await readFile(path.join(renameRepository, 'old'), 'utf8')).toBe('replacement\nextra\n')
      execFileSync('git', ['-C', renameRepository, 'reset', '--quiet', 'HEAD', '--', '.'])
      execFileSync('git', ['-C', renameRepository, 'add', 'old'])
      const retainedPreview = await client.copilotCommitDiffForRepository(renameRepository, 'new', {}, 'old')
      await client.repositoryStageRenamedLines(renameRepository, 'new', 'old', retainedPreview.basis, retainedPreview.hunks.flatMap(hunk => hunk.lines.filter(line => line.kind !== 'context').map(line => line.id)), true)
      expect(execFileSync('git', ['-C', renameRepository, 'show', ':old'], { encoding: 'utf8' })).toBe('replacement\nextra\n')
      execFileSync('git', ['-C', renameRepository, 'reset', '--quiet', 'HEAD', '--', '.'])
      const partialRenamePreview = await client.copilotCommitDiffForRepository(renameRepository, 'new', {}, 'old')
      await client.repositoryStageRenamedLines(renameRepository, 'new', 'old', partialRenamePreview.basis, partialRenamePreview.hunks.flatMap(hunk => hunk.lines.filter(line => line.kind !== 'context').map(line => line.id)))
      const partialSourcePreview = await client.repositoryWorkingDiff(renameRepository, 'old')
      await client.repositoryStageSelectedLines(renameRepository, 'old', partialSourcePreview.basis, [partialSourcePreview.hunks[0].lines[0].id])
      expect(execFileSync('git', ['-C', renameRepository, 'show', ':old'], { encoding: 'utf8' })).toBe('replacement\n')

      const commitRepository = path.join(directory, 'selected-commit')
      await client.request('init_repository', { path: commitRepository })
      execFileSync('git', ['-C', commitRepository, 'config', 'user.name', 'Test'])
      execFileSync('git', ['-C', commitRepository, 'config', 'user.email', 'test@example.invalid'])
      await writeFile(path.join(commitRepository, 'file'), 'initial\n')
      execFileSync('git', ['-C', commitRepository, 'add', 'file'])
      execFileSync('git', ['-C', commitRepository, 'commit', '-qm', 'initial'])
      await writeFile(path.join(commitRepository, 'file'), 'staged\n')
      execFileSync('git', ['-C', commitRepository, 'add', 'file'])
      const originalCommitIndex = await readFile(path.join(commitRepository, '.git', 'index'))
      await writeFile(path.join(commitRepository, 'file'), 'reviewed\n')
      const hookPath = path.join(commitRepository, '.git', 'hooks', 'pre-commit')
      await writeFile(hookPath, '#!/bin/sh\nexit 1\n')
      await chmod(hookPath, 0o700)
      await client.selectBrowserRepository(commitRepository)
      const selection = { paths: ['file'] }
      const review = await client.repositorySelectedCommitPreview(commitRepository, selection)
      const failedCheckpoint = await client.prepareSelectedCommit(commitRepository, selection, review.basis)
      const failedProgress: Array<{ hookName: string; status: string }> = []
      await expect(client.publishSelectedCommit(commitRepository, failedCheckpoint, 'reviewed commit\n', { onHookProgress: event => failedProgress.push(event) })).rejects.toThrow('requires inspection')
      expect(failedProgress).toContainEqual({ hookName: 'pre-commit', status: 'failed' })
      expect(await readFile(path.join(commitRepository, '.git', 'index'))).toEqual(originalCommitIndex)
      await client.closeUnchangedCommitCheckpoint(commitRepository, failedCheckpoint)
      expect(await client.listCommitCheckpoints(commitRepository)).toContainEqual(expect.objectContaining({ id: failedCheckpoint, state: 'closed' }))
      await writeFile(hookPath, '#!/bin/sh\nprintf started > .git/long-hook-started\nprintf "live-output\\n" >&2\nsleep 60\nprintf finished > .git/long-hook-finished\n')
      const cancelledCheckpoint = await client.prepareSelectedCommit(commitRepository, selection, review.basis)
      const commitAbort = new AbortController()
      const hookProgress: Array<{ hookName: string; status: string }> = []
      const liveOutput: string[] = []
      const pendingCommit = client.publishSelectedCommit(commitRepository, cancelledCheckpoint, 'reviewed commit\n', { signal: commitAbort.signal, onHookProgress: event => hookProgress.push(event), onOutput: event => liveOutput.push(new TextDecoder().decode(event.bytes)) })
      await waitFor(async () => (await readFile(path.join(commitRepository, '.git', 'long-hook-started'), 'utf8').catch(() => '')) === 'started')
      await waitFor(() => hookProgress.some(event => event.hookName === 'pre-commit' && event.status === 'started'))
      await waitFor(() => liveOutput.join('').includes('live-output'))
      commitAbort.abort()
      await expect(pendingCommit).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect commit checkpoints') })
      expect((await client.inspectCommitCheckpoint(commitRepository, cancelledCheckpoint)).state).toBe('publishing')
      expect(await readFile(path.join(commitRepository, '.git', 'index'))).toEqual(originalCommitIndex)
      await client.closeUnchangedCommitCheckpoint(commitRepository, cancelledCheckpoint)
      expect((await readdir(path.join(commitRepository, '.git', 'desktop-commit-checkpoints', cancelledCheckpoint))).some(name => name.startsWith('hook-trace-'))).toBe(false)
      expect(await readFile(path.join(commitRepository, '.git', 'long-hook-finished'), 'utf8').catch(() => '')).toBe('')
      await writeFile(hookPath, '#!/bin/sh\nprintf invoked > .git/commit-hook-ran\nprintf "hook output\\n" >&2\n')
      const commitCheckpoint = await client.prepareSelectedCommit(commitRepository, selection, review.basis)
      const completedProgress: Array<{ hookName: string; status: string }> = []
      const committed = await client.publishSelectedCommit(commitRepository, commitCheckpoint, 'reviewed commit\n', { onHookProgress: event => completedProgress.push(event) })
      expect(completedProgress).toContainEqual({ hookName: 'pre-commit', status: 'finished' })
      expect(execFileSync('git', ['-C', commitRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(committed.oid)
      expect(committed.stderr).toContain('hook output')
      expect(execFileSync('git', ['-C', commitRepository, 'show', 'HEAD:file'], { encoding: 'utf8' })).toBe('reviewed\n')
      expect(execFileSync('git', ['-C', commitRepository, 'show', ':file'], { encoding: 'utf8' })).toBe('reviewed\n')
      expect(await readFile(path.join(commitRepository, '.git', 'commit-hook-ran'), 'utf8')).toBe('invoked')
      expect(await readFile(path.join(commitRepository, '.git', 'desktop-commit-checkpoints', commitCheckpoint, 'index.before'))).toEqual(originalCommitIndex)
      expect((await readdir(path.join(commitRepository, '.git', 'desktop-commit-checkpoints', commitCheckpoint))).some(name => name.startsWith('hook-trace-'))).toBe(false)
      const configuredTrace = path.join(directory, 'user-trace')
      execFileSync('git', ['-C', commitRepository, 'config', 'trace2.eventTarget', configuredTrace])
      await writeFile(path.join(commitRepository, 'file'), 'next\n')
      const nextReview = await client.repositorySelectedCommitPreview(commitRepository, selection)
      const nextCheckpoint = await client.prepareSelectedCommit(commitRepository, selection, nextReview.basis)
      const suppressedProgress: Array<unknown> = []
      await client.publishSelectedCommit(commitRepository, nextCheckpoint, 'next commit\n', { onHookProgress: event => suppressedProgress.push(event) })
      expect(suppressedProgress).toEqual([])
      expect(execFileSync('git', ['-C', commitRepository, 'config', '--get', 'trace2.eventTarget'], { encoding: 'utf8' }).trim()).toBe(configuredTrace)

      const undoRepository = path.join(directory, 'undo-first')
      await client.request('init_repository', { path: undoRepository })
      await writeFile(path.join(undoRepository, 'file'), 'preserve')
      execFileSync('git', ['-C', undoRepository, 'add', 'file'])
      execFileSync('git', ['-C', undoRepository, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'first'])
      const initialOid = execFileSync('git', ['-C', undoRepository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
      await client.selectBrowserRepository(undoRepository)
      await writeFile(path.join(undoRepository, 'additional'), 'reviewed\n')
      const treeReview = await client.repositorySelectedCommitPreview(undoRepository, { paths: ['additional'] })
      expect(await client.verifySelectedIndex(undoRepository, treeReview)).toBe(false)
      execFileSync('git', ['-C', undoRepository, 'add', 'additional'])
      const indexBeforeTreeCheck = await readFile(path.join(undoRepository, '.git', 'index'))
      expect(await client.verifySelectedIndex(undoRepository, treeReview)).toBe(true)
      expect(await readFile(path.join(undoRepository, '.git', 'index'))).toEqual(indexBeforeTreeCheck)
      execFileSync('git', ['-C', undoRepository, 'reset', '--quiet', 'HEAD', '--', 'additional'])
      await expect(client.repositoryDeleteInitialHead(undoRepository, '0'.repeat(40))).rejects.toThrow()
      await client.repositoryDeleteInitialHead(undoRepository, initialOid)
      expect(await readFile(path.join(undoRepository, 'file'), 'utf8')).toBe('preserve')
      expect(spawnSync('git', ['-C', undoRepository, 'rev-parse', '--verify', 'HEAD']).status).not.toBe(0)
      await client.selectBrowserRepository(firstRepository)

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
    }, 180_000)

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
