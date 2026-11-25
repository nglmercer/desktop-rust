import { describe, expect, it } from 'vitest'
import { GitError } from '../../src/lib/git/core'
import {
  fetchFailureToGitError,
  isNativeFetchSamlReauthError,
} from '../../src/lib/git/fetch-error'

describe('Git module initialization', () => {
  it('preserves GitError identity across the core and fetch error import cycle', () => {
    const error = fetchFailureToGitError(
      { exitCode: 128, errorKind: 'https_saml_reauth_required' },
      '/repo',
      'origin'
    )

    expect(error).toBeInstanceOf(GitError)
    expect(isNativeFetchSamlReauthError(error)).toBe(true)
  })
})
