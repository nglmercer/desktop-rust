import { GitError as DugiteError } from 'dugite'
import { FetchErrorKind, FetchFailure } from '../../backend/client'
import { getDescriptionForError, GitError } from './core'

const errorKinds: Record<FetchErrorKind, DugiteError> = {
  ssh_key_unverified: DugiteError.SSHKeyAuditUnverified,
  ssh_authentication_failed: DugiteError.SSHAuthenticationFailed,
  ssh_permission_denied: DugiteError.SSHPermissionDenied,
  https_authentication_failed: DugiteError.HTTPSAuthenticationFailed,
  https_saml_reauth_required: DugiteError.HTTPSAuthenticationFailed,
  ssh_saml_reauth_required: DugiteError.SSHPermissionDenied,
  remote_disconnection: DugiteError.RemoteDisconnection,
  host_down: DugiteError.HostDown,
  https_repository_not_found: DugiteError.HTTPSRepositoryNotFound,
  ssh_repository_not_found: DugiteError.SSHRepositoryNotFound,
  lock_file_exists: DugiteError.LockFileAlreadyExists,
  bad_config_value: DugiteError.BadConfigValue,
  no_submodule_mapping: DugiteError.NoSubmoduleMapping,
  submodule_repository_does_not_exist: DugiteError.SubmoduleRepositoryDoesNotExist,
  invalid_submodule_sha: DugiteError.InvalidSubmoduleSHA,
}

export class NativeFetchError extends GitError {
  public constructor(
    failure: FetchFailure,
    repositoryPath: string,
    remote: string,
    public readonly samlReauthRequired: boolean
  ) {
    const gitError = failure.errorKind === null ? null : errorKinds[failure.errorKind]
    super(
      {
        exitCode: failure.exitCode,
        stdout: '',
        stderr: '',
        path: repositoryPath,
        gitError,
        gitErrorDescription: gitError === null ? null : getDescriptionForError(gitError, ''),
      },
      ['fetch', '--', remote],
      ''
    )
  }
}

/** Convert a completed native fetch failure into the existing Git error contract. */
export function fetchFailureToGitError(
  failure: FetchFailure,
  repositoryPath: string,
  remote: string
): GitError {
  return new NativeFetchError(
    failure,
    repositoryPath,
    remote,
    failure.errorKind === 'https_saml_reauth_required' || failure.errorKind === 'ssh_saml_reauth_required'
  )
}
