import { backend } from '../../backend/client'

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
  await backend().writeRepositoryCreationFile(
    path,
    'readme',
    defaultReadmeContents(name, description)
  )
}
