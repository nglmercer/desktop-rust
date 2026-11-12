import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  initialize: vi.fn(),
  branch: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    initializeDesktopRepository: mocks.initialize,
  }),
}))
vi.mock('../../src/lib/helpers/default-branch', () => ({
  getDefaultBranch: mocks.branch,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
import { initGitRepository } from '../../src/lib/git/init'

describe('repository initialization', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
    mocks.branch.mockResolvedValue('trunk')
  })

  it.each(['ws', 'wry'])(
    'uses the Rust host and configured branch on %s',
    async transport => {
      mocks.transport = transport
      await initGitRepository('/new/repo')
      expect(mocks.initialize).toHaveBeenCalledWith('/new/repo', 'trunk')
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('propagates a Rust failure without falling back to Git execution', async () => {
    mocks.initialize.mockRejectedValueOnce(new Error('already initialized'))
    await expect(initGitRepository('/new/repo')).rejects.toThrow(
      'already initialized'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves the Electron initializer', async () => {
    mocks.transport = 'none'
    await initGitRepository('/new/repo')
    expect(mocks.git).toHaveBeenCalledWith(
      ['-c', 'init.defaultBranch=trunk', 'init'],
      '/new/repo',
      'initGitRepository'
    )
  })
})
