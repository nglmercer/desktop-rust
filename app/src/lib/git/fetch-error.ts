import { GitError as DugiteError } from 'dugite'
import { FetchErrorKind, FetchFailure } from '../../backend/client'
import { getDescriptionForError, GitError } from './core'

const errorKinds: Record<FetchErrorKind, DugiteError> = {
  ssh_key_unverified: DugiteError.SSHKeyAuditUnverified,
  ssh_authentication_failed: DugiteError.SSHAuthenticationFailed,
  ssh_permission_denied: DugiteError.SSHPermissionDenied,
  https_authentication_failed: DugiteError.HTTPSAuthenticationFailed,
  remote_disconnection: DugiteError.RemoteDisconnection,
  host_down: DugiteError.HostDown,
  https_repository_not_found: DugiteError.HTTPSRepositoryNotFound,
  ssh_repository_not_found: DugiteError.SSHRepositoryNotFound,
  lock_file_exists: DugiteError.LockFileAlreadyExists,
}

/** Convert a completed native fetch failure into the existing Git error contract. */
export function fetchFailureToGitError(
  failure: FetchFailure,
  repositoryPath: string,
  remote: string
): GitError {
  const gitError = failure.errorKind === null ? null : errorKinds[failure.errorKind]
  return new GitError(
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
