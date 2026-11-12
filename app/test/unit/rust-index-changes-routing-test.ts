import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  read: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryIndexChanges: mocks.read,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))

import { getIndexChanges, IndexStatus } from '../../src/lib/git/diff-index'
import type { Repository } from '../../src/models/repository'
const repository = { path: '/repo' } as Repository

describe.each(['wry', 'ws'])('Rust staged index read through %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })

  it('maps staged statuses and does not invoke legacy Git', async () => {
    mocks.read.mockResolvedValue([
      { path: 'new\nname', status: 'A' },
      { path: 'conflict', status: 'U' },
    ])
    await expect(getIndexChanges(repository)).resolves.toEqual(
      new Map([
        ['new\nname', IndexStatus.Added],
        ['conflict', IndexStatus.Unmerged],
      ])
    )
    expect(mocks.git).not.toHaveBeenCalled()
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
  })

  it('fails closed on a backend error', async () => {
    const error = new Error('index unavailable')
    mocks.read.mockRejectedValue(error)
    await expect(getIndexChanges(repository)).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
