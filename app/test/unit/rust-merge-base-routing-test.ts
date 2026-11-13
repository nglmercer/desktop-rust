import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  base: vi.fn(),
  start: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryMergeBase: mocks.base,
    startRepositoryMerge: mocks.start,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
import { getMergeBase, merge, MergeResult } from '../../src/lib/git/merge'

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
  it.each([
    ['complete', MergeResult.Success],
    ['conflict', MergeResult.Failed],
    ['up_to_date', MergeResult.AlreadyUpToDate],
  ] as const)('routes reviewed merge result %s', async (outcome, result) => {
    const repository = { path: '/repo' } as Repository
    const expected = {
      sourceRef: 'refs/heads/-topic',
      sourceHead: 'a'.repeat(40),
      currentRef: 'refs/heads/main',
      currentHead: 'b'.repeat(40),
    }
    mocks.start.mockResolvedValue(outcome)
    await expect(
      merge(repository, '-topic', { squash: true, noVerify: true }, expected)
    ).resolves.toBe(result)
    expect(mocks.start).toHaveBeenCalledWith('/repo', {
      source_ref: expected.sourceRef,
      source_head: expected.sourceHead,
      current_ref: expected.currentRef,
      current_head: expected.currentHead,
      squash: true,
      no_verify: true,
    })
    expect(mocks.git).not.toHaveBeenCalled()
    await expect(merge(repository, '-topic')).rejects.toThrow(
      'Refresh the current and source branches'
    )
  })
})
