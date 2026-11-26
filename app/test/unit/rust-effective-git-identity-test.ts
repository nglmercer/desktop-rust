import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  identity: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryEffectiveGitIdentity: mocks.identity,
  }),
}))

import { getAuthorIdentity, getCommitterIdentity } from '../../src/lib/git/var'
import type { Repository } from '../../src/models/repository'

const repository = { path: '/new/repository' } as Repository

describe('effective Git identity', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
    mocks.identity.mockResolvedValue(
      'A Name <author@example.test> 1700000000 +0530\n'
    )
  })

  it.each(['ws', 'wry'])(
    'uses Git identity from %s for both roles',
    async transport => {
      mocks.transport = transport
      const author = await getAuthorIdentity(repository)
      const committer = await getCommitterIdentity(repository)
      expect(author).toMatchObject({
        name: 'A Name',
        email: 'author@example.test',
        tzOffset: 330,
      })
      expect(committer?.date.toISOString()).toBe('2023-11-14T22:13:20.000Z')
      expect(mocks.identity.mock.calls).toEqual([
        ['/new/repository', 'author'],
        ['/new/repository', 'committer'],
      ])
      expect(mocks.identity).toHaveBeenCalled()
    }
  )

  it('preserves missing and malformed identities while propagating transport errors', async () => {
    mocks.identity
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce('bad identity')
      .mockRejectedValueOnce(new Error('disconnected'))
    await expect(getAuthorIdentity(repository)).resolves.toBeNull()
    await expect(getAuthorIdentity(repository)).resolves.toBeNull()
    await expect(getAuthorIdentity(repository)).rejects.toThrow('disconnected')
  })

  it('forwards explicit environment overrides to the host operation', async () => {
    mocks.identity.mockResolvedValue(
      'Custom <custom@example.test> 1700000000 +0000\n'
    )
    const result = await getCommitterIdentity(repository, {
      env: { GIT_COMMITTER_NAME: 'Custom' },
    })
    expect(result?.name).toBe('Custom')
    expect(mocks.identity).toHaveBeenCalledWith(
      '/new/repository',
      'committer',
      {
        GIT_COMMITTER_NAME: 'Custom',
      }
    )
  })
})
