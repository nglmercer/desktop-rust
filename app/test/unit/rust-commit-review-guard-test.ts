import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  unstage: vi.fn(),
  prepare: vi.fn(),
  stage: vi.fn(),
  verify: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/lib/git/core', () => ({
  git: mocks.git,
  parseCommitSHA: () => 'committed',
}))
vi.mock('../../src/lib/git/update-index', () => ({
  preparePartialStagingDiffs: mocks.prepare,
  stageFiles: mocks.stage,
}))
vi.mock('../../src/lib/git/reset', () => ({ unstageAll: mocks.unstage }))
vi.mock('../../src/lib/git/stage', () => ({
  stageManualConflictResolution: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ verifySelectedIndex: mocks.verify }),
}))
import { createCommit } from '../../src/lib/git/commit'

describe('selected commit review guard', () => {
  const repository = { path: '/repo' } as Repository
  const selectedReview = {
    tree: 'a'.repeat(40),
    head: 'b'.repeat(40),
    branch: 'refs/heads/main',
  }
  beforeEach(() => {
    vi.resetAllMocks()
    mocks.prepare.mockResolvedValue(undefined)
    mocks.unstage.mockResolvedValue(true)
    mocks.stage.mockResolvedValue(undefined)
    mocks.git.mockResolvedValue({ stdout: '' })
  })
  it('does not invoke commit when the staged tree or HEAD differs', async () => {
    mocks.verify.mockResolvedValue(false)
    await expect(
      createCommit(repository, 'message', [], { selectedReview })
    ).rejects.toThrow('changed since review')
    expect(mocks.verify).toHaveBeenCalledWith('/repo', selectedReview)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('checks the staged tree before invoking commit', async () => {
    mocks.verify.mockResolvedValue(true)
    await expect(
      createCommit(repository, 'message', [], { selectedReview })
    ).resolves.toBe('committed')
    expect(mocks.verify.mock.invocationCallOrder[0]).toBeGreaterThan(
      mocks.stage.mock.invocationCallOrder[0]
    )
    expect(mocks.verify.mock.invocationCallOrder[0]).toBeLessThan(
      mocks.git.mock.invocationCallOrder[0]
    )
  })
})
