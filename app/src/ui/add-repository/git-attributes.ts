import { writeFile } from 'fs/promises'
import { join } from 'path'
import { backend } from '../../backend/client'

/**
 * Write the .gitAttributes file to the given repository
 */
export async function writeGitAttributes(path: string): Promise<void> {
  const fullPath = join(path, '.gitattributes')
  const contents =
    '# Auto detect text files and perform LF normalization\n* text=auto\n'
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await backend().writeRepositoryCreationFile(path, 'git_attributes', contents)
    return
  }
  await writeFile(fullPath, contents)
}
