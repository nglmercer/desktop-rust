import { describe, expect, it, vi } from 'vitest'
import { GitError as DugiteError } from 'dugite'
import { GitError, isAuthFailureError } from '../../src/lib/git/core'
import { fetchFailureToGitError } from '../../src/lib/git/fetch-error'
import { ErrorWithMetadata } from '../../src/lib/error-with-metadata'
import { GitHubRepository } from '../../src/models/github-repository'
import { Owner } from '../../src/models/owner'
import { Repository } from '../../src/models/repository'
import { PopupType } from '../../src/models/popup'
import { samlReauthRequired } from '../../src/ui/dispatcher/error-handlers'
import type { Dispatcher } from '../../src/ui/dispatcher'

vi.mock('../../src/lib/trampoline/trampoline-environment', () => ({
  withTrampolineEnv: vi.fn(),
}))
vi.mock('../../src/lib/hooks/with-hooks-env', () => ({ withHooksEnv: vi.fn() }))
vi.mock('../../src/lib/custom-integration', () => ({}))

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

  it.each([
    ['remote_repository_unavailable', 'remote repository could not be accessed'],
    ['remote_ref_not_found', 'requested reference was not found'],
    ['fetch_non_fast_forward', 'cannot be fast-forwarded'],
    ['fetch_tag_clobber', 'overwrite an existing local tag'],
    ['host_resolution_failed', 'remote host could not be resolved'],
  ] as const)('shows a safe explanation for %s', (errorKind, description) => {
    const error = fetchFailureToGitError({ exitCode: 1, errorKind }, '/repo', 'origin')
    expect(error.result.gitError).toBeNull()
    expect(error.message.toLowerCase()).toContain(description)
    expect(error.result.stderr).toBe('')
  })

  it.each([
    ['bad_config_value', DugiteError.BadConfigValue, 'configuration'],
    ['no_submodule_mapping', DugiteError.NoSubmoduleMapping, 'submodule'],
    ['submodule_repository_does_not_exist', DugiteError.SubmoduleRepositoryDoesNotExist, 'submodule'],
    ['invalid_submodule_sha', DugiteError.InvalidSubmoduleSHA, 'submodule'],
  ] as const)('reports %s without exposing Git output', (errorKind, expected, description) => {
    const error = fetchFailureToGitError({ exitCode: 128, errorKind }, '/repo', 'origin')
    expect(error.result.gitError).toBe(expected)
    expect(error.message.toLowerCase()).toContain(description)
    expect(error.result.stderr).toBe('')
  })

  it('opens SAML reauthorization for the reviewed repository owner without remote text', async () => {
    const owner = new Owner('example', 'https://api.github.com', 1)
    const repository = new Repository('/repo', 1, new GitHubRepository('project', owner, 1), false)
    const error = fetchFailureToGitError(
      { exitCode: 128, errorKind: 'https_saml_reauth_required' }, '/repo', 'origin'
    )
    const showPopup = vi.fn()
    const result = await samlReauthRequired(
      new ErrorWithMetadata(error, { repository }),
      { showPopup } as unknown as Dispatcher
    )
    expect(result).toBeNull()
    expect(showPopup).toHaveBeenCalledWith(expect.objectContaining({
      type: PopupType.SAMLReauthRequired,
      organizationName: 'example',
      endpoint: 'https://api.github.com',
    }))
    expect(error.result.stderr).toBe('')
  })
})
