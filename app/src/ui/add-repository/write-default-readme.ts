import { writeFile } from 'fs/promises'
import * as Path from 'path'
import { backend } from '../../backend/client'

const DefaultReadmeName = 'README.md'

function defaultReadmeContents(name: string, description?: string): string {
  return description !== undefined
    ? `# ${name}\n${description}\n`
    : `# ${name}\n`
}

/**
 * Write the default README to the repository with the given name at the path.
 */
export async function writeDefaultReadme(
  path: string,
  name: string,
  description?: string
): Promise<void> {
  const fullPath = Path.join(path, DefaultReadmeName)
  const contents = defaultReadmeContents(name, description)
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await backend().writeRepositoryCreationFile(path, 'readme', contents)
    return
  }
  await writeFile(fullPath, contents)
}
