import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry', abort: vi.fn(), git: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<void>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, abortRepositoryGitOperation: mocks.abort }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/status', () => ({ getStatus: vi.fn() }))
vi.mock('../../src/lib/git/update-index', () => ({ stageFiles: vi.fn() }))
vi.mock('../../src/lib/git/rev-list', () => ({ getCommitsBetweenCommits: vi.fn(), getCommitsInRange: vi.fn(), revRange: vi.fn() }))
vi.mock('../../src/lib/git/stage', () => ({ stageManualConflictResolution: vi.fn() }))
vi.mock('../../src/lib/git', () => ({ getCommit: vi.fn() }))
vi.mock('../../src/lib/merge', () => ({ merge: vi.fn() }))
vi.mock('../../src/lib/rebase', () => ({ formatRebaseValue: vi.fn() }))
vi.mock('../../src/lib/git/multi-operation-terminal-output', () => ({ createMultiOperationTerminalOutputCallback: vi.fn() }))

import { abortCherryPick } from '../../src/lib/git/cherry-pick'
import { abortRebase } from '../../src/lib/git/rebase'
import { abortMerge } from '../../src/lib/git/merge'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/repo' } as Repository

describe.each(['wry', 'ws'])('Rust operation abort through %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport; mocks.abort.mockResolvedValue(undefined) })

  it.each([
    ['merge', abortMerge],
    ['rebase', abortRebase],
    ['cherry_pick', abortCherryPick],
  ] as const)('uses the scoped native %s abort', async (kind, abort) => {
    await abort(repository)
    expect(mocks.abort).toHaveBeenCalledWith('/repo', kind)
    expect(mocks.scope).toHaveBeenCalledOnce()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates failure without invoking the old Git path', async () => {
    mocks.abort.mockRejectedValue(new Error('operation changed'))
    await expect(abortMerge(repository)).rejects.toThrow('operation changed')
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
