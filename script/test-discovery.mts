import { readdir, stat } from 'node:fs/promises'
import { join } from 'node:path'

/** Find test files passed explicitly or under the requested directories. */
export async function findTestFilesIn(
  paths: readonly string[]
): Promise<string[]> {
  const files: string[] = []
  for (const path of paths) {
    const entry = await stat(path)
    if (entry.isFile()) {
      files.push(path)
      continue
    }

    for (const file of await readdir(path, { recursive: true }).then(x =>
      x
        .filter(f => /(?:-test|\.test)\.(ts|tsx|js|jsx|mts|mjs)$/.test(f))
        .map(f => join(path, f))
    )) {
      files.push(file)
    }
  }
  return files
}
