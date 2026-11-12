import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  mutate: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    mutateRepositoryTag: mocks.mutate,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: vi.fn(),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
import { createTag, deleteTag } from '../../src/lib/git/tag'

describe.each(['wry', 'ws'])('Rust tag mutations via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
    mocks.mutate.mockResolvedValue(undefined)
  })
  it('creates and deletes annotated tags within the repository mutation scope', async () => {
    const repository = { path: '/repo' } as Repository
    const target = 'HEAD'
    await createTag(repository, 'release', target)
    await deleteTag(repository, 'release')
    expect(mocks.mutate).toHaveBeenNthCalledWith(1, '/repo', {
      method: 'tag_create_annotated',
      name: 'release',
      target,
    })
    expect(mocks.mutate).toHaveBeenNthCalledWith(2, '/repo', {
      method: 'tag_delete',
      name: 'release',
    })
    expect(mocks.scope).toHaveBeenCalledTimes(2)
    expect(mocks.git).not.toHaveBeenCalled()
    const failure = new Error('failed')
    mocks.mutate.mockRejectedValueOnce(failure)
    await expect(deleteTag(repository, 'release')).rejects.toBe(failure)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
