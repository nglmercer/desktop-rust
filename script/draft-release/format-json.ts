import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

/** Format release JSON with the same Biome configuration used by the project. */
export function formatJson(
  source: string,
  filename = 'changelog.json'
): string {
  return execFileSync(
    process.execPath,
    [
      require.resolve('@biomejs/biome/bin/biome'),
      'format',
      `--config-path=${join(__dirname, '..', 'biome-release.json')}`,
      `--stdin-file-path=${filename}`,
    ],
    { input: source, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 }
  )
}

/** Format each file before writing any, so a parse failure leaves them intact. */
export function formatJsonFiles(paths: ReadonlyArray<string>): void {
  const formatted = paths.map(path => ({
    path,
    content: formatJson(readFileSync(path, 'utf8'), path),
  }))
  for (const { path, content } of formatted) {
    writeFileSync(path, content)
  }
}
