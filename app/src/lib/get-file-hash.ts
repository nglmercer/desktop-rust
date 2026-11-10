import { createHash } from 'crypto'
import { createReadStream } from 'fs'

/**
 * Whether the Rust shell serves this renderer. There the file is hashed in
 * blocks by Rust and never crosses to the renderer.
 */
const hashedByShell = (): boolean => {
  const shell = (globalThis as any).__desktop
  return shell?.platform === 'wry' && typeof shell.invoke === 'function'
}

/** Hex digest of a file, computed by the Rust shell. */
const hashInShell = (path: string, type: 'sha1' | 'sha256'): Promise<string> =>
  (globalThis as any).__desktop.invoke('desktop.io.fs-hash', {
    path,
    algorithm: type,
  })

/** Hex digest of a file, read as a stream (Electron). */
const hashInStream = (path: string, type: 'sha1' | 'sha256') =>
  new Promise<string>((resolve, reject) => {
    const hash = createHash(type)

    hash.on('finish', () => resolve(hash.digest('hex')))
    hash.on('error', reject)

    createReadStream(path).on('error', reject).pipe(hash)
  })

/**
 * Calculates the hex encoded hash digest of a given file on disk.
 */
export const getFileHash = (path: string, type: 'sha1' | 'sha256'): Promise<string> =>
  hashedByShell() ? hashInShell(path, type) : hashInStream(path, type)
