import { backend } from '../../backend/client'

export const DefaultGitDescription =
  "Unnamed repository; edit this file 'description' to name the repository.\n"

/** Get the repository's description from the .git/description file. */
export async function getGitDescription(
  repositoryPath: string
): Promise<string> {
  return backend().repositoryGitDescription(repositoryPath)
}

/** Write a .git/description file to the repository. */
export async function writeGitDescription(
  repositoryPath: string,
  description: string
): Promise<void> {
  await backend().setRepositoryGitDescription(repositoryPath, description)
}
