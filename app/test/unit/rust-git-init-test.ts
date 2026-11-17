import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  initialize: vi.fn(),
  branch: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    initializeDesktopRepository: mocks.initialize,
  }),
}))
vi.mock('../../src/lib/helpers/default-branch', () => ({
  getDefaultBranch: mocks.branch,
}))
import { initGitRepository } from '../../src/lib/git/init'

describe('repository initialization', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.branch.mockResolvedValue('trunk')
  })

  it('uses the Rust host and configured branch', async () => {
    await initGitRepository('/new/repo')
    expect(mocks.initialize).toHaveBeenCalledWith('/new/repo', 'trunk')
  })

  it('propagates a Rust failure without falling back to Git execution', async () => {
    mocks.initialize.mockRejectedValueOnce(new Error('already initialized'))
    await expect(initGitRepository('/new/repo')).rejects.toThrow(
      'already initialized'
    )
  })
})
