import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry', state: vi.fn(), context: vi.fn(), git: vi.fn(), commit: vi.fn(), status: vi.fn(), stage: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<unknown>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, repositoryCherryPickState: mocks.state, repositoryStatusContext: mocks.context }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/status', () => ({ getStatus: mocks.status }))
vi.mock('../../src/lib/git/update-index', () => ({ stageFiles: mocks.stage }))
vi.mock('../../src/lib/git/rev-list', () => ({ getCommitsInRange: vi.fn(), revRange: vi.fn() }))
vi.mock('../../src/lib/git/stage', () => ({ stageManualConflictResolution: vi.fn() }))
vi.mock('../../src/lib/git', () => ({ getCommit: mocks.commit }))
vi.mock('../../src/lib/merge', () => ({ merge: vi.fn() }))

import { continueCherryPick, CherryPickResult, getCherryPickSnapshot, isCherryPickHeadFound } from '../../src/lib/git/cherry-pick'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo', resolvedGitDir: '/not-readable' } as Repository
const head = 'a'.repeat(40)

describe.each(['wry', 'ws'])('Rust cherry-pick state through %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport })

  it('returns null when no cherry-pick is in progress', async () => {
    mocks.state.mockResolvedValue(null)
    await expect(getCherryPickSnapshot(repository)).resolves.toBeNull()
    expect(mocks.git).not.toHaveBeenCalled()
    expect(mocks.commit).not.toHaveBeenCalled()
  })

  it('uses the native sequencer without renderer filesystem reads', async () => {
    mocks.state.mockResolvedValue({ head, sequence: { head, abortSafety: head, todo: 'pick abc1234 first\npick def5678 second\n' } })
    const snapshot = await getCherryPickSnapshot(repository)
    expect(snapshot?.remainingCommits).toEqual([{ sha: 'abc1234', summary: 'first' }, { sha: 'def5678', summary: 'second' }])
    expect(snapshot?.targetBranchUndoSha).toBe(head)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('handles a single cherry-pick and refuses a changed head', async () => {
    mocks.state.mockResolvedValue({ head, sequence: null })
    mocks.commit.mockResolvedValue({ sha: head, summary: 'one commit' })
    expect((await getCherryPickSnapshot(repository))?.commits).toEqual([{ sha: head, summary: 'one commit' }])
    vi.clearAllMocks()
    mocks.state.mockResolvedValueOnce({ head, sequence: null }).mockResolvedValueOnce({ head: 'b'.repeat(40), sequence: null })
    await expect(getCherryPickSnapshot(repository)).resolves.toBeNull()
    expect(mocks.commit).not.toHaveBeenCalled()
  })

  it('does not continue or create an empty commit after the cherry-pick ends', async () => {
    mocks.status.mockResolvedValue({ workingDirectory: { files: [] } })
    mocks.context.mockResolvedValue({ isCherryPickingHeadFound: false })
    await expect(continueCherryPick(repository, [])).resolves.toBe(CherryPickResult.UnableToStart)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('awaits the active-state check and stops safely on backend errors', async () => {
    mocks.context.mockResolvedValueOnce({ isCherryPickingHeadFound: false }).mockRejectedValueOnce(new Error('disconnected'))
    await expect(isCherryPickHeadFound(repository)).resolves.toBe(false)
    await expect(isCherryPickHeadFound(repository)).resolves.toBe(false)
  })
})
