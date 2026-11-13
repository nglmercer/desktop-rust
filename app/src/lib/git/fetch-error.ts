import { GitError as DugiteError } from 'dugite'
import { FetchErrorKind, FetchFailure } from '../../backend/client'
import { getDescriptionForError, GitError } from './core'

const errorKinds: Record<FetchErrorKind, DugiteError | null> = {
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
  remote_repository_unavailable: null,
  remote_ref_not_found: null,
  fetch_non_fast_forward: null,
  fetch_tag_clobber: null,
  host_resolution_failed: null,
}

// These fixed descriptions preserve useful fetch diagnostics without exposing
// remote-controlled paths, URLs, or credentials to the renderer.
const fetchDescriptions: Partial<Record<FetchErrorKind, string>> = {
  remote_repository_unavailable: 'The remote repository could not be accessed. Check that it exists and that you have access.',
  remote_ref_not_found: 'The requested reference was not found on the remote.',
  fetch_non_fast_forward: 'The fetch would overwrite a local reference that cannot be fast-forwarded.',
  fetch_tag_clobber: 'The fetch would overwrite an existing local tag.',
  host_resolution_failed: 'The remote host could not be resolved. Check your connection and remote URL.',
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
  remote: string
): GitError {
  const gitError = failure.errorKind === null ? null : errorKinds[failure.errorKind]
  const error = new GitError(
    {
      exitCode: failure.exitCode,
      stdout: '',
      stderr: '',
      path: repositoryPath,
      gitError,
      gitErrorDescription: failure.errorKind === null ? null :
        fetchDescriptions[failure.errorKind] ?? (gitError === null ? null : getDescriptionForError(gitError, '')),
    },
    ['fetch', '--', remote],
    ''
  )
  if (failure.errorKind === 'https_saml_reauth_required' || failure.errorKind === 'ssh_saml_reauth_required') {
    samlReauthErrors.add(error)
  }
  return error
}
