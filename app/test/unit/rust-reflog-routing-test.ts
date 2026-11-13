import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  read: vi.fn(),
  checkouts: vi.fn(),
  authors: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryRendererReflog: mocks.read,
    repositoryBranchCheckouts: mocks.checkouts,
    repositoryAuthors: mocks.authors,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git, GitError: {} }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))

import { getBranchCheckouts, getRecentBranches } from '../../src/lib/git/reflog'
import { getStashes } from '../../src/lib/git/stash'
import { getAuthors } from '../../src/lib/git/log'

const repository = { path: '/repo' } as Repository

describe.each(['wry', 'ws'])('Rust reflog reads via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })
  it('reads author identities in bounded batches and detects duplicates across batches', async () => {
    const revisions = Array.from({ length: 501 }, (_, index) => index.toString(16).padStart(40, '0'))
    mocks.authors.mockImplementation(async (_path, values: string[]) => values.map(oid => ({ oid: oid.toLowerCase(), identity: 'Test <test@example.invalid> 100 +0000' })))
    const authors = await getAuthors(repository, revisions)
    expect(authors).toHaveLength(501)
    expect(authors[0].name).toBe('Test')
    expect(mocks.authors).toHaveBeenCalledTimes(2)
    expect(mocks.authors.mock.calls[0][1]).toHaveLength(500)
    expect(mocks.authors.mock.calls[1][1]).toHaveLength(1)
    mocks.authors.mockClear()
    const duplicate = [...revisions.slice(0, 500), revisions[10].toUpperCase()]
    await expect(getAuthors(repository, duplicate)).rejects.toThrow('Commit to author mismatch')
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('preserves recent checkout order and excludes renamed branches', async () => {
    mocks.read.mockResolvedValue(
      'abc HEAD@{0}: Branch: renamed refs/heads/old to refs/heads/new\nabc HEAD@{1}: checkout: moving from main to old\nabc HEAD@{2}: checkout: moving from main to topic\n'
    )
    await expect(getRecentBranches(repository, 2)).resolves.toEqual([
      'new',
      'topic',
    ])
    expect(mocks.read).toHaveBeenCalledWith('/repo', 'branches')
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('preserves the newest checkout date per branch and propagates errors', async () => {
    const after = new Date('2024-01-01T00:00:00.000Z')
    mocks.checkouts
      .mockResolvedValueOnce(
        [
          `${'a'.repeat(40)} HEAD@{2024-02-03 10:00:00 +0000} checkout: moving from main to topic`,
          `${'b'.repeat(64)} HEAD@{2024-02-02 10:00:00 +0000} checkout: moving from topic to another`,
          `${'c'.repeat(40)} HEAD@{2024-02-01 10:00:00 +0000} checkout: moving from another to topic`,
        ].join('\n')
      )
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error('history unavailable'))
    expect([...(await getBranchCheckouts(repository, after))]).toEqual([
      ['topic', new Date('2024-02-03T10:00:00Z')],
      ['another', new Date('2024-02-02T10:00:00Z')],
    ])
    expect(mocks.checkouts).toHaveBeenCalledWith('/repo', after)
    expect((await getBranchCheckouts(repository, after)).size).toBe(0)
    await expect(getBranchCheckouts(repository, after)).rejects.toThrow(
      'history unavailable'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('preserves stash entries, counts, empty reflogs, and errors', async () => {
    const data = [
      'refs/stash@{0}',
      'a'.repeat(40),
      'On main: !!GitHub_Desktop<topic>',
      'b'.repeat(40),
      'c'.repeat(40),
      'refs/stash@{1}',
      'd'.repeat(40),
      'On main: manual',
      'e'.repeat(40),
      'f'.repeat(40),
      '',
    ].join('\0')
    mocks.read
      .mockResolvedValueOnce(data)
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error('unavailable'))
    await expect(getStashes(repository)).resolves.toMatchObject({
      stashEntryCount: 1,
      desktopEntries: [
        {
          name: 'refs/stash@{0}',
          branchName: 'topic',
          stashSha: 'a'.repeat(40),
        },
      ],
    })
    await expect(getStashes(repository)).resolves.toEqual({
      desktopEntries: [],
      stashEntryCount: 0,
    })
    await expect(getStashes(repository)).rejects.toThrow('unavailable')
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
