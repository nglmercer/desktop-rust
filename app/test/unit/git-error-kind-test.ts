import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { GitError } from 'dugite'
import { GitErrorKind } from '../../src/lib/git/error-kind'

describe('GitErrorKind', () => {
  it('preserves the numeric legacy parser vocabulary', () => {
    const entries = (value: object) =>
      Object.fromEntries(
        Object.entries(value).filter(([, kind]) => typeof kind === 'number')
      )
    assert.deepStrictEqual(entries(GitErrorKind), entries(GitError))
  })
})
