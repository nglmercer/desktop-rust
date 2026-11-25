import { GitErrorKind } from './error-kind'
import { FetchErrorKind, FetchFailure } from '../../backend/client'
import { getDescriptionForError, GitError } from './core'

const errorKinds: Record<FetchErrorKind, GitErrorKind | null> = {
  ssh_key_unverified: GitErrorKind.SSHKeyAuditUnverified,
  ssh_authentication_failed: GitErrorKind.SSHAuthenticationFailed,
  ssh_permission_denied: GitErrorKind.SSHPermissionDenied,
  https_authentication_failed: GitErrorKind.HTTPSAuthenticationFailed,
  https_saml_reauth_required: GitErrorKind.HTTPSAuthenticationFailed,
  ssh_saml_reauth_required: GitErrorKind.SSHPermissionDenied,
  remote_disconnection: GitErrorKind.RemoteDisconnection,
  host_down: GitErrorKind.HostDown,
  https_repository_not_found: GitErrorKind.HTTPSRepositoryNotFound,
  ssh_repository_not_found: GitErrorKind.SSHRepositoryNotFound,
  lock_file_exists: GitErrorKind.LockFileAlreadyExists,
  bad_config_value: GitErrorKind.BadConfigValue,
  no_submodule_mapping: GitErrorKind.NoSubmoduleMapping,
  submodule_repository_does_not_exist:
    GitErrorKind.SubmoduleRepositoryDoesNotExist,
  invalid_submodule_sha: GitErrorKind.InvalidSubmoduleSHA,
  remote_repository_unavailable: null,
  remote_ref_not_found: null,
  fetch_non_fast_forward: null,
  fetch_tag_clobber: null,
  host_resolution_failed: null,
}

// These fixed descriptions preserve useful fetch diagnostics without exposing
// remote-controlled paths, URLs, or credentials to the renderer.
const fetchDescriptions: Partial<Record<FetchErrorKind, string>> = {
  remote_repository_unavailable:
    'The remote repository could not be accessed. Check that it exists and that you have access.',
  remote_ref_not_found: 'The requested reference was not found on the remote.',
  fetch_non_fast_forward:
    'The fetch would overwrite a local reference that cannot be fast-forwarded.',
  fetch_tag_clobber: 'The fetch would overwrite an existing local tag.',
  host_resolution_failed:
    'The remote host could not be resolved. Check your connection and remote URL.',
}

// Keep the marker separate from the error's public diagnostics. Creating a
// subclass at module load time would also read GitError during the existing
// core/trampoline import cycle, before that class has been initialized.
const samlReauthErrors = new WeakSet<GitError>()

export function isNativeFetchSamlReauthError(error: GitError): boolean {
  return samlReauthErrors.has(error)
}

/** Convert a completed native fetch failure into the existing Git error contract. */
export function fetchFailureToGitError(
  failure: FetchFailure,
  repositoryPath: string,
  remote: string,
  operation: 'fetch' | 'pull' = 'fetch'
): GitError {
  const gitError =
    failure.errorKind === null ? null : errorKinds[failure.errorKind]
  const error = new GitError(
    {
      exitCode: failure.exitCode,
      stdout: '',
      stderr: '',
      path: repositoryPath,
      gitError,
      gitErrorDescription:
        failure.errorKind === null
          ? null
          : (fetchDescriptions[failure.errorKind] ??
            (gitError === null ? null : getDescriptionForError(gitError, ''))),
    },
    [operation, '--', remote],
    ''
  )
  if (
    failure.errorKind === 'https_saml_reauth_required' ||
    failure.errorKind === 'ssh_saml_reauth_required'
  ) {
    samlReauthErrors.add(error)
  }
  return error
}
