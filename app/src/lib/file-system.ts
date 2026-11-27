import { randomUUID } from '../web/random-uuid'
import { join } from 'path'
import { hostTemporaryDirectory } from './host-environment'

/**
 * Get a path to a temp file using the given name. Note that the file itself
 * will not be created.
 */
export const getTempFilePath = (name: string) =>
  join(
    hostTemporaryDirectory(),
    `${name}-${randomUUID().replaceAll('-', '').slice(0, 16)}`
  )
