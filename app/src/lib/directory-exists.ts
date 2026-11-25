import { resolve } from 'path'
import { backend } from '../backend/client'

/**
 * Helper method to stat a path and check both that it exists and that it's
 * a directory.
 */
export const directoryExists = async (path: string) =>
  path.length !== 0 && (await backend().directoryExists(resolve(path)))
