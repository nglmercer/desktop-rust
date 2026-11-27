import { sha256Hex } from './sha256'
import { GitHubRepository } from '../models/github-repository'

/** Method to create the url for viewing a commit on dotcom */
export function createCommitURL(
  gitHubRepository: GitHubRepository,
  SHA: string,
  filePath?: string
): string | null {
  const baseURL = gitHubRepository.htmlURL

  if (baseURL === null) {
    return null
  }

  if (filePath === undefined) {
    return `${baseURL}/commit/${SHA}`
  }

  const fileHash = sha256Hex(filePath)
  const fileSuffix = '#diff-' + fileHash

  return `${baseURL}/commit/${SHA}${fileSuffix}`
}
