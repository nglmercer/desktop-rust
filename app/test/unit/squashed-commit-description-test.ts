import { describe, it, vi } from 'vitest'
import assert from 'node:assert'
import type { ITrailer } from '../../src/lib/git/interpret-trailers'
import { Commit } from '../../src/models/commit'
import { CommitIdentity } from '../../src/models/commit-identity'
import { getSquashedCommitDescription } from '../../src/lib/squash/squashed-commit-description'

// Commit construction uses the real trailer parser without executing Git.
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))
vi.mock('../../src/lib/git/config', () => ({ getConfigValue: vi.fn() }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: vi.fn(),
}))

describe('getSquashedCommitDescription', () => {
  const mockCoAuthorTrailers = [
    { token: 'Co-Authored-By', value: 'test <test>' },
  ]
  it('builds squashed commit descriptions - no coauthors provided', async () => {
    const commits: Commit[] = [
      buildTestCommit('summary1', 'desc1', []),
      buildTestCommit('summary2', 'desc2', []),
    ]

    const squashOnto = buildTestCommit('ontoSummary', 'ontoDesc', [])

    const desc = getSquashedCommitDescription(commits, squashOnto)
    assert.equal(desc, 'ontoDesc\n\nsummary1\n\ndesc1\n\nsummary2\n\ndesc2')
  })

  it('builds squashed commit descriptions that do not include coauthors', async () => {
    const commits: Commit[] = [
      buildTestCommit('summary1', 'desc1', mockCoAuthorTrailers),
      buildTestCommit('summary2', 'desc2', mockCoAuthorTrailers),
    ]

    const squashOnto = buildTestCommit(
      'ontoSummary',
      'ontoDesc',
      mockCoAuthorTrailers
    )

    const desc = getSquashedCommitDescription(commits, squashOnto)
    assert.equal(desc, 'ontoDesc\n\nsummary1\n\ndesc1\n\nsummary2\n\ndesc2')
  })

  it('builds squashed commit descriptions with whitespace trimmed', async () => {
    const commits: Commit[] = [
      buildTestCommit('summary1    ', 'desc1   ', mockCoAuthorTrailers),
      buildTestCommit('summary2\n', 'desc2\n', mockCoAuthorTrailers),
    ]

    const squashOnto = buildTestCommit(
      'ontoSummary',
      'ontoDesc  \n',
      mockCoAuthorTrailers
    )

    const desc = getSquashedCommitDescription(commits, squashOnto)
    assert.equal(desc, 'ontoDesc\n\nsummary1\n\ndesc1\n\nsummary2\n\ndesc2')
  })
})

function buildTestCommit(
  summary: string,
  body: string,
  trailers: ReadonlyArray<ITrailer>
): Commit {
  const author = new CommitIdentity('test', 'test', new Date())

  return new Commit(
    'test',
    'test',
    summary,
    body,
    author,
    author,
    [],
    trailers,
    []
  )
}
