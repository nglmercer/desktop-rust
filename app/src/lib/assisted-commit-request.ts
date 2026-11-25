import { ICopilotAssistedCommitRequest } from '../models/copilot-assisted-commit'
import { WorkingDirectoryFileChange } from '../models/status'
import { Repository } from '../models/repository'
import { backend } from '../backend/client'

/** Freeze UI inputs before consent or other awaited preflight work. Not a Git snapshot. */
export function freezeAssistedCommitRequest(
  request: ICopilotAssistedCommitRequest
): ICopilotAssistedCommitRequest {
  return Object.freeze({
    ...request,
    files: Object.freeze(
      request.files.map(file =>
        Object.freeze(
          new WorkingDirectoryFileChange(
            file.path,
            Object.freeze({ ...file.status }),
            file.selection
          )
        )
      )
    ),
    trailers: Object.freeze(
      request.trailers.map(trailer => Object.freeze({ ...trailer }))
    ),
  })
}

/** Reject stale warning/consent continuations rather than expanding their selection or options. */
export function assistedCommitRequestsEqual(
  left: ICopilotAssistedCommitRequest,
  right: ICopilotAssistedCommitRequest
): boolean {
  return (
    left.skipCommitHooks === right.skipCommitHooks &&
    left.signOffCommits === right.signOffCommits &&
    left.allowEmptyCommit === right.allowEmptyCommit &&
    JSON.stringify(left.trailers) === JSON.stringify(right.trailers) &&
    left.files.length === right.files.length &&
    left.files.every((file, index) => {
      const other = right.files[index]
      return (
        file.path === other.path &&
        JSON.stringify(file.status) === JSON.stringify(other.status) &&
        file.selection.equals(other.selection)
      )
    })
  )
}

/**
 * Local preflight freshness, without reading file contents, diffing, or capturing a Git snapshot.
 *
 * Consent/keychain waits must not authorize newly edited selected files. These
 * conservative metadata versions are not executor authority; capture still
 * freezes and verifies exact bytes and identities after consent.
 */
export async function getAssistedCommitInputVersions(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>
): Promise<ReadonlyArray<string>> {
  if (files.length === 0) {
    return []
  }
  const versions = await backend().repositoryWorkingFileMetadataVersions(
    repository.path,
    files.map(file => file.path)
  )
  return versions.map(version => version ?? 'missing')
}

/** Text anchors are unsafe after inode, file type, or executable-mode changes. */
export function assistedCommitInputIdentityMatches(
  before: string | undefined,
  current: string | undefined
): boolean {
  return (
    before !== undefined &&
    current !== undefined &&
    before !== 'missing' &&
    current !== 'missing' &&
    before.split(':').slice(0, 3).join(':') ===
      current.split(':').slice(0, 3).join(':')
  )
}
