import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { FileChange, AppFileStatusKind } from '../../src/models/status'
import { DiffType } from '../../src/models/diff'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws', compare: vi.fn(), git: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<unknown>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, repositoryRendererCompareDiff: mocks.compare, repositorySubmoduleDisplayUrl: async () => null }) }))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/custom-integration', () => ({}))
vi.mock('keytar', () => ({ default: {} }))

import { getBranchMergeBaseDiff, getCommitRangeDiff } from '../../src/lib/git/diff'

let root: string
let raw: Buffer
let first: string
let last: string
const repository = { path: '/repo', gitHubRepository: null } as Repository
const file = new FileChange('[a].txt', { kind: AppFileStatusKind.Modified })

beforeAll(() => {
  root = mkdtempSync(path.join(tmpdir(), 'desktop-comparison-routing-'))
  const git = (...args: string[]) => execFileSync('git', ['-C', root, ...args])
  git('init', '-q', '-b', 'main'); git('config', 'user.name', 'Test'); git('config', 'user.email', 'test@example.invalid'); git('config', 'commit.gpgsign', 'false')
  writeFileSync(path.join(root, '[a].txt'), 'first\n'); git('add', '--', '.'); git('commit', '-qm', 'first')
  first = git('rev-parse', 'HEAD').toString().trim()
  writeFileSync(path.join(root, '[a].txt'), 'second\n'); git('add', '--', '.'); git('commit', '-qm', 'second')
  last = git('rev-parse', 'HEAD').toString().trim()
  raw = git('diff', '--merge-base', '--patch-with-raw', '-z', '--no-color', '--end-of-options', first, last, '--', ':(literal)[a].txt')
})
afterAll(() => { if (root) rmSync(root, { recursive: true, force: true }) })

describe.each(['wry', 'ws'])('Rust comparison diff via %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport; mocks.compare.mockReset() })

  it('routes merge-base comparisons and retains literal path/rename inputs', async () => {
    mocks.compare.mockResolvedValue(raw)
    const result = await getBranchMergeBaseDiff(repository, file, first, last, true, last)
    expect(result.kind).toBe(DiffType.Text)
    expect(mocks.compare).toHaveBeenCalledWith('/repo', first, last, true, '[a].txt', undefined, true)
    expect(mocks.git).not.toHaveBeenCalled()
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
    const renamed = new FileChange('[b].txt', { kind: AppFileStatusKind.Renamed, oldPath: '[a].txt', renameIncludesModifications: false })
    await getBranchMergeBaseDiff(repository, renamed, first, last, false, last)
    expect(mocks.compare).toHaveBeenLastCalledWith('/repo', first, last, true, '[b].txt', '[a].txt', false)
  })

  it('retries an initial commit with the empty tree but propagates other failures', async () => {
    mocks.compare.mockResolvedValueOnce(null).mockResolvedValueOnce(raw)
    await getCommitRangeDiff(repository, file, [first])
    expect(mocks.compare).toHaveBeenNthCalledWith(1, '/repo', `${first}^`, first, false, '[a].txt', undefined, false)
    expect(mocks.compare).toHaveBeenNthCalledWith(2, '/repo', '4b825dc642cb6eb9a060e54bf8d69288fbee4904', first, false, '[a].txt', undefined, false)
    mocks.compare.mockReset().mockRejectedValueOnce(new Error('transport failed'))
    await expect(getCommitRangeDiff(repository, file, [first])).rejects.toThrow('transport failed')
    expect(mocks.compare).toHaveBeenCalledTimes(1)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('keeps oversized text unrenderable and still renders submodule commit IDs', async () => {
    const prefix = Buffer.from('-Subproject commit oldsha\n+Subproject commit newsha\n')
    mocks.compare.mockResolvedValue({ kind: 'unrenderable', prefix })
    expect((await getBranchMergeBaseDiff(repository, file, first, last, false, last)).kind).toBe(DiffType.Unrenderable)
    expect((await getCommitRangeDiff(repository, file, [first])).kind).toBe(DiffType.Unrenderable)
    const submodule = new FileChange('sub', { kind: AppFileStatusKind.Modified, submoduleStatus: { commitChanged: true, modifiedChanges: false, untrackedChanges: false } })
    const result = await getBranchMergeBaseDiff(repository, submodule, first, last, false, last)
    expect(result.kind).toBe(DiffType.Submodule)
    if (result.kind === DiffType.Submodule) {
      expect(result.oldSHA).toBe('oldsha')
      expect(result.newSHA).toBe('newsha')
    }
  })
})
