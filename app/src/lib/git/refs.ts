import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { resolve } from 'path'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Format a local branch in the ref syntax, ensuring situations when the branch
 * is ambiguous are handled.
 */
export function formatAsLocalRef(name: string): string {
  if (name.startsWith('heads/')) {
    return `refs/${name}`
  } else if (!name.startsWith('refs/heads/')) {
    return `refs/heads/${name}`
  } else {
    return name
  }
}

/**
 * Read a symbolic ref from the repository.
 *
 * @returns the canonical ref, if found, or `null` if `ref` cannot be found or
 *          is not a symbolic ref
 */
export async function getSymbolicRef(
  repository: Repository,
  ref: string
): Promise<string | null> {
  const read = () =>
    backend().repositorySymbolicRef(resolve(repository.path), ref)
  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(repository.path, 'read', read, true)
    : read()
}
