import { beforeEach, describe, expect, it, vi } from 'vitest'
import { AppFileStatusKind, GitStatusEntry, type WorkingDirectoryFileChange } from '../../src/models/status'
import { ManualConflictResolution } from '../../src/models/manual-conflict-resolution'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  resolve: vi.fn(),
  add: vi.fn(),
  remove: vi.fn(),
  checkout: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<void>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, repositoryResolveManualConflict: mocks.resolve }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/git/add', () => ({ addConflictedFile: mocks.add }))
vi.mock('../../src/lib/git/rm', () => ({ removeConflictedFile: mocks.remove }))
vi.mock('../../src/lib/git/checkout', () => ({ checkoutConflictedFile: mocks.checkout }))

import { stageManualConflictResolution } from '../../src/lib/git/stage'

const repository = { path: '/repo' } as Repository
const file = (us: GitStatusEntry, them: GitStatusEntry) => ({ path: 'file', status: { kind: AppFileStatusKind.Conflicted, entry: { us, them } } }) as WorkingDirectoryFileChange

describe.each(['wry', 'ws'])('Rust manual conflict resolution through %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport; mocks.resolve.mockResolvedValue(undefined) })

  it.each([
    [GitStatusEntry.UpdatedButUnmerged, GitStatusEntry.UpdatedButUnmerged, ManualConflictResolution.ours, 'ours'],
    [GitStatusEntry.Added, GitStatusEntry.Added, ManualConflictResolution.theirs, 'theirs'],
    [GitStatusEntry.Added, GitStatusEntry.Deleted, ManualConflictResolution.ours, 'edited'],
    [GitStatusEntry.Added, GitStatusEntry.Deleted, ManualConflictResolution.theirs, 'delete'],
  ])('maps selected index sides without using legacy Git', async (us, them, resolution, choice) => {
    await stageManualConflictResolution(repository, file(us, them), resolution)
    expect(mocks.resolve).toHaveBeenCalledWith('/repo', 'file', choice, { ours: us !== GitStatusEntry.Deleted, theirs: them !== GitStatusEntry.Deleted })
    expect(mocks.scope).toHaveBeenCalledOnce()
    expect(mocks.add).not.toHaveBeenCalled()
    expect(mocks.remove).not.toHaveBeenCalled()
    expect(mocks.checkout).not.toHaveBeenCalled()
  })

  it('does not hide resolution failures or retry via legacy Git', async () => {
    mocks.resolve.mockRejectedValue(new Error('conflict changed'))
    await expect(stageManualConflictResolution(repository, file(GitStatusEntry.Added, GitStatusEntry.Deleted), ManualConflictResolution.ours)).rejects.toThrow('conflict changed')
    expect(mocks.add).not.toHaveBeenCalled()
  })
})
