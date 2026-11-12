import { access } from 'fs/promises'
import constant from 'lodash/constant'
import { resolve } from 'path'
import { backend } from '../backend/client'

/**
 * Returns a value indicating whether or not the provided path exists (as in
 * whether it's visible to the current process or not).
 */
export const pathExists = (path: string) => {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    return path.length === 0
      ? Promise.resolve(false)
      : backend().pathExists(resolve(path))
  }
  return access(path).then(constant(true), constant(false))
}
