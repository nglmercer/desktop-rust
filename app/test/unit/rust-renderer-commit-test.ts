import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  commit: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryRendererCommit: mocks.commit,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
import { getCommit } from '../../src/lib/git/log'

const repository = { path: '/repo' } as Repository
const sha = 'a'.repeat(40)
const raw = Buffer.from(
  [
    sha,
    'aaaaaaa',
    'Subject',
    'Body\n\nCo-authored-by: Other <other@example.invalid>\n',
    'Author <author@example.invalid> 1700000000 +0200',
    'Committer <committer@example.invalid> 1700000010 -0500',
    'b'.repeat(40),
    'Co-authored-by: Other <other@example.invalid>\n',
    'HEAD -> main, tag: v1',
  ].join('\0') + '\0'
).toString('base64')

describe.each(['ws', 'wry'])('Rust commit lookup over %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })
  it('preserves identities, message, trailers, parents and tags without the legacy runner', async () => {
    mocks.commit.mockResolvedValue(raw)
    const commit = await getCommit(repository, sha)
    expect(commit).toMatchObject({
      sha,
      shortSha: 'aaaaaaa',
      summary: 'Subject',
      author: {
        name: 'Author',
        email: 'author@example.invalid',
        tzOffset: 120,
      },
      committer: { name: 'Committer', tzOffset: -300 },
      parentSHAs: ['b'.repeat(40)],
      tags: ['v1'],
    })
    expect(commit?.coAuthors[0]).toMatchObject({
      name: 'Other',
      email: 'other@example.invalid',
    })
    expect(mocks.git).not.toHaveBeenCalled()
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
  })
  it('preserves missing revisions and propagates backend failures', async () => {
    mocks.commit
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error('read failed'))
    await expect(getCommit(repository, 'missing')).resolves.toBeNull()
    await expect(getCommit(repository, sha)).rejects.toThrow('read failed')
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
