import { resolve } from 'path'
import { backend } from '../../backend/client'

export type RepositoryType =
  | { kind: 'bare' }
  | { kind: 'regular'; topLevelWorkingDirectory: string; gitDir: string }
  | { kind: 'missing' }
  | { kind: 'unsafe'; path: string }

/**
 * Attempts to fulfill the work of isGitRepository and isBareRepository while
 * requiring only one Git process to be spawned.
 *
 * Returns 'bare', 'regular', 'unsafe', or 'missing' if the repository couldn't
 * be found. An unsafe path is a canonical directory containing the requested
 * path, verified against Git's ownership diagnostic.
 */
export async function getRepositoryType(path: string): Promise<RepositoryType> {
  return path.length === 0
    ? { kind: 'missing' }
    : backend().repositoryType(resolve(path))
}
