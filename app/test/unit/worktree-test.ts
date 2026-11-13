import assert from 'node:assert'
import { describe, it, vi } from 'vitest'

import { WorktreeEntry, getWorktreeAriaLabel } from '../../src/models/worktree'

// The worktree label uses Commit's real SHA formatter without invoking Git.
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))
vi.mock('../../src/lib/git/config', () => ({ getConfigValue: vi.fn() }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: vi.fn() }))

function worktree(overrides: Partial<WorktreeEntry> = {}): WorktreeEntry {
  return {
    path: '/repo-feature',
    head: 'abc1234abc1234abc1234abc1234abc1234abc123',
    branch: 'refs/heads/feature',
    isDetached: false,
    type: 'linked',
    isLocked: false,
    isPrunable: false,
    ...overrides,
  }
}

describe('getWorktreeAriaLabel', () => {
  it('names the worktree and its branch', () => {
    assert.equal(getWorktreeAriaLabel(worktree()), 'repo-feature, feature')
  })

  it('announces a missing worktree', () => {
    assert.equal(
      getWorktreeAriaLabel(worktree({ isPrunable: true })),
      'repo-feature, missing, feature'
    )
  })
})
