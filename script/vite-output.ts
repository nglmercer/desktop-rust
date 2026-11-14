import {
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  realpathSync,
  rmSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import * as path from 'node:path'

/** Resolve symlinked ancestors even if the final output does not exist yet. */
function resolvedPath(value: string): string {
  let current = path.resolve(value)
  const rest: string[] = []
  for (;;) {
    try {
      return path.join(realpathSync(current), ...rest.reverse())
    } catch (error) {
      if (
        !(
          error &&
          typeof error === 'object' &&
          'code' in error &&
          error.code === 'ENOENT'
        )
      )
        throw error
      const parent = path.dirname(current)
      if (parent === current) throw error
      rest.push(path.basename(current))
      current = parent
    }
  }
}

function contains(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate)
  return (
    relative === '' ||
    (!path.isAbsolute(relative) &&
      relative !== '..' &&
      !relative.startsWith(`..${path.sep}`))
  )
}

/** Return the Vite bundle directory shared by the renderer, worker and bootstrap. */
export function viteOutputDirectory(
  repo: string,
  env: NodeJS.ProcessEnv = process.env
): string {
  const development = path.resolve(repo, 'out/vite-dist')
  const configured = env.DESKTOP_VITE_DIST
  if (configured === undefined) return development
  if (!path.isAbsolute(configured) || /[\0\r\n]/.test(configured)) {
    throw new Error(
      'DESKTOP_VITE_DIST must be an absolute path without control characters'
    )
  }
  const output = path.resolve(configured)
  if (path.parse(output).root === output) {
    throw new Error('DESKTOP_VITE_DIST cannot be a filesystem root')
  }
  const devReal = resolvedPath(development)
  const outputReal = resolvedPath(output)
  if (contains(devReal, outputReal) || contains(outputReal, devReal)) {
    throw new Error('DESKTOP_VITE_DIST must not overlap the development bundle')
  }
  return output
}

/** Refuse to overwrite any pre-existing files in an isolated bundle. */
export function requireEmptyViteOutput(output: string): void {
  try {
    const metadata = lstatSync(output)
    if (
      !metadata.isDirectory() ||
      metadata.isSymbolicLink() ||
      readdirSync(output).length !== 0
    ) {
      throw new Error('The isolated Vite output must be a new, empty directory')
    }
  } catch (error) {
    if (
      error &&
      typeof error === 'object' &&
      'code' in error &&
      error.code === 'ENOENT'
    )
      return
    throw error
  }
}

/** A secondary build may add files, but must never overwrite existing ones. */
export function requireAbsentViteOutputFiles(
  output: string,
  files: readonly string[]
): void {
  for (const file of files) {
    const destination = path.join(output, file)
    try {
      lstatSync(destination)
    } catch (error) {
      if (
        error &&
        typeof error === 'object' &&
        'code' in error &&
        error.code === 'ENOENT'
      )
        continue
      throw error
    }
    throw new Error(`The isolated Vite output already contains ${file}`)
  }
}

/** Create and later remove only a fresh temporary bundle owned by this build. */
export function createIsolatedViteOutput(): {
  directory: string
  dispose: () => void
} {
  const temporary = mkdtempSync(path.join(tmpdir(), 'desktop-vite-production-'))
  const identity = lstatSync(temporary)
  const directory = path.join(temporary, 'bundle')
  try {
    mkdirSync(directory, { mode: 0o700 })
  } catch (error) {
    const current = lstatSync(temporary)
    if (
      current.isDirectory() &&
      !current.isSymbolicLink() &&
      current.dev === identity.dev &&
      current.ino === identity.ino
    ) {
      rmSync(temporary, { recursive: true })
    }
    throw error
  }
  return {
    directory,
    dispose: () => {
      let current
      try {
        current = lstatSync(temporary)
      } catch (error) {
        if (
          error &&
          typeof error === 'object' &&
          'code' in error &&
          error.code === 'ENOENT'
        )
          return
        throw error
      }
      if (
        !current.isDirectory() ||
        current.isSymbolicLink() ||
        current.dev !== identity.dev ||
        current.ino !== identity.ino
      ) {
        throw new Error(
          `Production build directory changed; retained ${temporary}`
        )
      }
      rmSync(temporary, { recursive: true })
    },
  }
}
