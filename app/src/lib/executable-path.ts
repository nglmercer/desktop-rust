import { backend } from '../backend/client'

interface IWhichOptions {
  readonly path?: string
  readonly nothrow?: boolean
}

const notFound = (command: string) =>
  Object.assign(new Error(`not found: ${command}`), { code: 'ENOENT' })

/** Resolve the first executable matching `command` in a PATH-style list. */
export async function which(
  command: string,
  options: IWhichOptions = {}
): Promise<string | null> {
  const resolved = await backend().resolveExecutable(command, options.path)
  if (resolved !== null || options.nothrow === true) {
    return resolved
  }
  throw notFound(command)
}
