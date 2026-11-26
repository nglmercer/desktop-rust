import { delimiter, join } from 'path'
import { access, stat, constants } from '../web/fs-promises'

interface IWhichOptions {
  readonly path?: string
  readonly nothrow?: boolean
}

const windowsExtensions = () => {
  const value = process.env.PATHEXT ?? '.EXE;.CMD;.BAT;.COM'
  const extensions = value.split(';').filter(extension => extension.length > 0)
  return extensions.length > 0 ? extensions : ['.EXE', '.CMD', '.BAT', '.COM']
}

const commandCandidates = (command: string) => {
  if (!__WIN32__) {
    return [command]
  }
  const extensions = windowsExtensions()
  const lower = command.toLowerCase()
  if (extensions.some(extension => lower.endsWith(extension.toLowerCase()))) {
    return [command]
  }
  return [
    command,
    ...extensions.map(extension => `${command}${extension}`),
  ].filter((candidate, index, all) => all.indexOf(candidate) === index)
}

const executable = async (path: string): Promise<boolean> => {
  try {
    const metadata = await stat(path)
    if (!metadata.isFile()) {
      return false
    }
    if (__WIN32__) {
      return true
    }
    await access(path, constants.X_OK)
    return true
  } catch {
    return false
  }
}

const notFound = (command: string) =>
  Object.assign(new Error(`not found: ${command}`), { code: 'ENOENT' })

/** Resolve the first executable matching `command` in a PATH-style list. */
export async function which(
  command: string,
  options: IWhichOptions = {}
): Promise<string | null> {
  const hasPathSeparator = command.includes('/') || command.includes('\\')
  const candidates: string[] = []

  if (hasPathSeparator) {
    candidates.push(command)
  } else {
    const paths = (options.path ?? process.env.PATH ?? '').split(delimiter)
    for (const directory of paths) {
      for (const candidate of commandCandidates(command)) {
        candidates.push(join(directory, candidate))
      }
    }
  }

  for (const candidate of candidates) {
    if (await executable(candidate)) {
      return candidate
    }
  }

  if (options.nothrow === true) {
    return null
  }
  throw notFound(command)
}
