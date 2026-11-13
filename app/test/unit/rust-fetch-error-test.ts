import { describe, expect, it, vi } from 'vitest'
import { GitError as DugiteError } from 'dugite'
import { GitError, isAuthFailureError } from '../../src/lib/git/core'
import { fetchFailureToGitError } from '../../src/lib/git/fetch-error'

vi.mock('../../src/lib/trampoline/trampoline-environment', () => ({
  withTrampolineEnv: vi.fn(),
}))
vi.mock('../../src/lib/hooks/with-hooks-env', () => ({ withHooksEnv: vi.fn() }))

describe('native fetch errors', () => {
  it('preserves the existing authentication error contract without remote output', () => {
    const error = fetchFailureToGitError(
      { exitCode: 128, errorKind: 'https_authentication_failed' },
      '/repo',
      'origin'
    )
    expect(error).toBeInstanceOf(GitError)
    expect(error.result.gitError).toBe(DugiteError.HTTPSAuthenticationFailed)
    expect(isAuthFailureError(error.result.gitError!)).toBe(true)
    expect(error.result.stderr).toBe('')
    expect(error.message).toContain('Authentication failed')
  })

  it('reports unclassified Git failures without inventing an authentication error', () => {
    const error = fetchFailureToGitError({ exitCode: 128, errorKind: null }, '/repo', 'origin')
    expect(error.result.gitError).toBeNull()
    expect(error.message).toContain('exit code 128')
  })
})
