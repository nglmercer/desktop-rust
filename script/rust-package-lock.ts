import { randomUUID } from 'node:crypto'
import {
  closeSync,
  fstatSync,
  lstatSync,
  openSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs'

/** Acquire one cooperative lock; never infer that an existing lock is stale. */
export function acquireRustPackageLock(file: string): () => void {
  let fd: number
  try {
    fd = openSync(file, 'wx', 0o600)
  } catch (error) {
    if (
      error &&
      typeof error === 'object' &&
      'code' in error &&
      error.code === 'EEXIST'
    ) {
      throw new Error(
        `Rust package is in use or a lock was retained: ${file}. Inspect its owner before removing it.`
      )
    }
    throw error
  }
  const identity = fstatSync(fd)
  const token = randomUUID()
  const release = () => {
    closeSync(fd)
    const current = lstatSync(file)
    if (
      !current.isFile() ||
      current.isSymbolicLink() ||
      current.dev !== identity.dev ||
      current.ino !== identity.ino ||
      !readFileSync(file, 'utf8').includes(`"token":"${token}"`)
    ) {
      throw new Error(
        `Rust package lock changed; refusing to remove it: ${file}`
      )
    }
    unlinkSync(file)
  }
  try {
    writeFileSync(
      fd,
      `${JSON.stringify({
        pid: process.pid,
        token,
        created: new Date().toISOString(),
      })}\n`
    )
  } catch (error) {
    try {
      release()
    } catch {}
    throw error
  }
  return release
}
