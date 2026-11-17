import * as Path from 'path'
import type { Repository } from '../../models/repository'
import { backend } from '../../backend/client'

/**
 * Read the contents of the repository .gitignore.
 *
 * Returns a promise which will either be rejected or resolved
 * with the contents of the file. If there's no .gitignore file
 * in the repository root the promise will resolve with null.
 */
export async function readGitIgnoreAtRoot(
  repository: Repository
): Promise<string | null> {
  return backend().repositoryGitIgnore(Path.resolve(repository.path), {
    type: 'read',
  })
}

/** Persist the given content to the repository root .gitignore. */
export async function saveGitIgnore(
  repository: Repository,
  text: string
): Promise<void> {
  await backend().repositoryGitIgnore(Path.resolve(repository.path), {
    type: 'save',
    text,
  })
}

/** Add the given pattern or patterns to the root gitignore file */
export async function appendIgnoreRule(
  repository: Repository,
  patterns: string | string[]
): Promise<void> {
  await backend().repositoryGitIgnore(Path.resolve(repository.path), {
    type: 'append',
    text: Array.isArray(patterns) ? patterns.join('\n') : patterns,
  })
}

/**
 * Convenience method to add the given file path(s) to the repository's gitignore.
 *
 * The file path will be escaped before adding.
 */
export async function appendIgnoreFile(
  repository: Repository,
  filePath: string | string[]
): Promise<void> {
  if (Array.isArray(filePath)) {
    const escapedFilePaths = filePath.map(path =>
      escapeGitSpecialCharacters(path)
    )

    return appendIgnoreRule(repository, escapedFilePaths)
  }

  const escapedFilePath = escapeGitSpecialCharacters(filePath)
  return appendIgnoreRule(repository, escapedFilePath)
}

/** Escapes a string from special characters used in a gitignore file */
export function escapeGitSpecialCharacters(pattern: string): string {
  const specialCharacters = /[[\]!*#?]/g

  return pattern.replaceAll(specialCharacters, match => {
    return '\\' + match
  })
}
