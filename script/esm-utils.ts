import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

/** Resolve the directory containing an ES module without relying on CommonJS. */
export function directoryOf(importMetaUrl: string): string {
  return path.dirname(fileURLToPath(importMetaUrl))
}

/** Whether the current module is the process entry point. */
export function isMainModule(importMetaUrl: string): boolean {
  const entry = process.argv[1]
  return (
    entry !== undefined &&
    pathToFileURL(path.resolve(entry)).href === importMetaUrl
  )
}
