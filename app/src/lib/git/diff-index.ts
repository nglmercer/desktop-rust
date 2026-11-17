import type { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Possible statuses of an entry in Git, see the git diff-index
 * man page for additional details.
 */
export enum IndexStatus {
  Unknown = 0,
  Added,
  Copied,
  Deleted,
  Modified,
  Renamed,
  TypeChanged,
  Unmerged,
}

/**
 * Index statuses excluding renames and copies.
 *
 * Used when invoking diff-index with rename detection explicitly turned
 * off.
 */
export type NoRenameIndexStatus =
  | IndexStatus.Added
  | IndexStatus.Deleted
  | IndexStatus.Modified
  | IndexStatus.TypeChanged
  | IndexStatus.Unmerged
  | IndexStatus.Unknown

function getIndexStatus(status: string) {
  switch (status[0]) {
    case 'A':
      return IndexStatus.Added
    case 'C':
      return IndexStatus.Copied
    case 'D':
      return IndexStatus.Deleted
    case 'M':
      return IndexStatus.Modified
    case 'R':
      return IndexStatus.Renamed
    case 'T':
      return IndexStatus.TypeChanged
    case 'U':
      return IndexStatus.Unmerged
    case 'X':
      return IndexStatus.Unknown
    default:
      throw new Error(`Unknown index status: ${status}`)
  }
}

function getNoRenameIndexStatus(status: string): NoRenameIndexStatus {
  const parsed = getIndexStatus(status)

  switch (parsed) {
    case IndexStatus.Copied:
    case IndexStatus.Renamed:
      throw new Error(
        `Invalid index status for no-rename index status: ${parsed}`
      )
  }

  return parsed
}

/** The SHA for the null tree. */
export const NullTreeSHA = '4b825dc642cb6eb9a060e54bf8d69288fbee4904'

/**
 * Get a list of files which have recorded changes in the index as compared to
 * HEAD along with the type of change.
 *
 * @param repository The repository for which to retrieve the index changes.
 */
export async function getIndexChanges(
  repository: Repository
): Promise<Map<string, NoRenameIndexStatus>> {
  const read = () => backend().repositoryIndexChanges(repository.path)
  const entries =
    backend().transportKind === 'wry'
      ? await withRepositoryGitOperation(repository.path, 'read', read, true)
      : await read()
  return new Map(
    entries.map(entry => [entry.path, getNoRenameIndexStatus(entry.status)])
  )
}
