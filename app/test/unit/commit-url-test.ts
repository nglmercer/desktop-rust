import assert from 'node:assert'
import { describe, it } from 'vitest'
import { createCommitURL } from '../../src/lib/commit-url'
import { sha256Hex } from '../../src/lib/sha256'
import { GitHubRepository } from '../../src/models/github-repository'
import { Owner } from '../../src/models/owner'

const repository = new GitHubRepository(
  'repository',
  new Owner('owner', 'https://api.github.com', 1),
  1,
  false,
  'https://github.com/owner/repository'
)

describe('createCommitURL', () => {
  it('creates commit URLs without file anchors', () => {
    assert.strictEqual(
      createCommitURL(repository, 'abc123'),
      'https://github.com/owner/repository/commit/abc123'
    )
  })

  it('creates byte-compatible UTF-8 diff anchors', () => {
    const filePath = 'src/área/📁 file.txt'
    assert.strictEqual(
      createCommitURL(repository, 'abc123', filePath),
      `https://github.com/owner/repository/commit/abc123#diff-${sha256Hex(
        filePath
      )}`
    )
  })

  it('returns null when the repository has no web URL', () => {
    const local = new GitHubRepository(
      'repository',
      new Owner('owner', 'https://api.github.com', 1),
      1
    )
    assert.strictEqual(createCommitURL(local, 'abc123', 'file.txt'), null)
  })
})
