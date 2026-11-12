import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  base: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryMergeBase: mocks.base,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
import { getMergeBase } from '../../src/lib/git/merge'

describe.each(['wry', 'ws'])('Rust merge-base lookup via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })
  it('returns the base or null, and propagates backend failures', async () => {
    const repository = { path: '/repo' } as Repository
    const oid = 'a'.repeat(40)
    mocks.base
      .mockResolvedValueOnce(oid)
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error('failed'))
    await expect(getMergeBase(repository, 'HEAD', 'topic')).resolves.toBe(oid)
    await expect(
      getMergeBase(repository, 'missing', 'topic')
    ).resolves.toBeNull()
    await expect(getMergeBase(repository, 'HEAD', 'topic')).rejects.toThrow(
      'failed'
    )
    expect(mocks.base).toHaveBeenCalledWith('/repo', 'HEAD', 'topic')
    expect(mocks.git).not.toHaveBeenCalled()
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 3 : 0)
  })
})
