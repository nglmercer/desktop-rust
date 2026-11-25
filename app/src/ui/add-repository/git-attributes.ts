import { backend } from '../../backend/client'

/**
 * Write the .gitAttributes file to the given repository
 */
export async function writeGitAttributes(path: string): Promise<void> {
  await backend().writeRepositoryCreationFile(
    path,
    'git_attributes',
    '# Auto detect text files and perform LF normalization\n* text=auto\n'
  )
}
