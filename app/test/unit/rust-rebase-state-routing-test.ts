import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry', state: vi.fn(), git: vi.fn(), commits: vi.fn(), status: vi.fn(), stage: vi.fn(), continueRebase: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<unknown>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, repositoryRebaseState: mocks.state, continueRepositoryRebase: mocks.continueRebase }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/status', () => ({ getStatus: mocks.status }))
vi.mock('../../src/lib/git/update-index', () => ({ stageFiles: mocks.stage }))
vi.mock('../../src/lib/git/rev-list', () => ({ getCommitsBetweenCommits: mocks.commits }))
vi.mock('../../src/lib/git/stage', () => ({ stageManualConflictResolution: vi.fn() }))
vi.mock('../../src/lib/merge', () => ({ merge: vi.fn() }))
vi.mock('../../src/lib/rebase', () => ({ formatRebaseValue: (value: number) => Math.round(value * 100) / 100 }))

import { continueRebase, getRebaseInternalState, getRebaseSnapshot, RebaseResult } from '../../src/lib/git/rebase'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo', resolvedGitDir: '/not-readable' } as Repository
const hash = 'a'.repeat(40)
const progress = { original: `${hash}\n`, branch: 'refs/heads/topic\n', onto: `${hash}\n`, current: '2\n', total: '4\n' }

describe.each(['wry', 'ws'])('Rust rebase state through %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport })

  it('reads state and computes progress without renderer filesystem access', async () => {
    mocks.state.mockResolvedValue({ head: hash, progress })
    mocks.commits.mockResolvedValue([{ sha: 'one', summary: 'first' }, { sha: 'two', summary: 'second' }])
    await expect(getRebaseInternalState(repository)).resolves.toEqual({ originalBranchTip: hash, targetBranch: 'topic', baseBranchTip: hash })
    expect((await getRebaseSnapshot(repository))?.progress).toMatchObject({ value: 0.5, position: 2, totalCommitCount: 4, currentCommitSummary: 'second' })
    expect(mocks.commits).toHaveBeenCalledWith(repository, hash, hash)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('returns null for a stopped or incomplete rebase', async () => {
    mocks.state.mockResolvedValueOnce(null).mockResolvedValueOnce({ head: hash, progress: { ...progress, current: null } })
    await expect(getRebaseInternalState(repository)).resolves.toBeNull()
    await expect(getRebaseSnapshot(repository)).resolves.toBeNull()
    expect(mocks.commits).not.toHaveBeenCalled()
  })

  it('does not continue after rebase state disappears', async () => {
    mocks.status.mockResolvedValue({ workingDirectory: { files: [] } })
    mocks.state.mockResolvedValue(null)
    await expect(continueRebase(repository, [])).resolves.toBe(RebaseResult.Aborted)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it.each([true, false])('continues a reviewed rebase with empty=%s', async empty => {
    mocks.state.mockResolvedValue({ head: hash, progress })
    mocks.status.mockResolvedValue({ workingDirectory: { files: empty ? [] : [{ status: { kind: 'modified' } }] } })
    mocks.continueRebase.mockResolvedValue('complete')
    await expect(continueRebase(repository, [])).resolves.toBe(RebaseResult.CompletedWithoutError)
    expect(mocks.continueRebase).toHaveBeenCalledWith('/repo', hash, empty, false)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('rejects a custom editor before staging', async () => {
    await expect(continueRebase(repository, [], new Map(), { gitEditor: 'custom' })).rejects.toThrow('Custom rebase editors')
    expect(mocks.stage).not.toHaveBeenCalled()
  })
})
