import { ICloneProgress } from '../../models/progress'
import { CloneOptions } from '../../models/clone-options'
import { CloneProgressParser } from '../progress'
import type { IGitOutput, IGitProgress } from '../progress/git'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import { backend, type ICloneTrampolineEnvironment } from '../../backend/client'
import { getDefaultBranch } from '../helpers/default-branch'
import { backendRemoteProxy, envForProxy } from './environment'
import { withTrampolineEnv } from '../trampoline/trampoline-environment'
import { homedir } from '../../web/os'
import * as Path from 'path'
import { unescape as decodeFilePath } from 'querystring'
import { Repository } from '../../models/repository'
import { getRepositoryType } from './rev-parse'
import { listWorktrees } from './worktree'
import {
  canonicalMutationPath,
  getAssistedCommitProtectedPaths,
  withoutRepositoryGitAccess,
  withRepositoryGitOperation,
  withRepositoryGitResourceMutation,
} from './repository-operation'

const unsupportedCloneProtocols: ReadonlyArray<string> = ['ext', 'ext.exe']

/** Resolve the filesystem source read by Git's local and file transports. */
export function getLocalCloneSourcePath(
  url: string,
  platform: 'posix' | 'win32' = __WIN32__ ? 'win32' : 'posix',
  cwd: string = __dirname
): string | null {
  const paths = platform === 'win32' ? Path.win32 : Path.posix
  if (url.startsWith('file://')) {
    // Git decodes before parsing; unlike a browser URL, #, ? and + stay in the path.
    const source = decodeFilePath(url.slice('file://'.length))
    if (platform === 'win32') {
      if (/^[a-z]:/i.test(source)) {
        return paths.resolve(cwd, source)
      }
      if (!source.startsWith('/') && /[\\/]/.test(source)) {
        // Git for Windows retains authorities (including localhost) as UNC servers.
        return paths.resolve(cwd, `//${source}`)
      }
    }
    const start = source.indexOf('/')
    if (start === -1) {
      return null
    }
    // POSIX Git ignores authorities; a Windows /C:/ root denotes the local drive.
    const path = source.slice(start)
    return paths.resolve(
      cwd,
      platform === 'win32' && /^\/[a-z]:/i.test(path) ? path.slice(1) : path
    )
  }
  const colon = url.indexOf(':')
  const slash = url.indexOf('/')
  if (
    colon !== -1 &&
    (slash === -1 || colon < slash) &&
    !(platform === 'win32' && /^[a-z]:/i.test(url))
  ) {
    return null
  }
  return paths.resolve(cwd, url)
}

async function withLocalCloneSourceRead(
  path: string,
  operation: () => Promise<void>,
  tryGitSuffix = true
): Promise<void> {
  return withRepositoryGitOperation(path, 'read', async () => {
    const type = await getRepositoryType(path)
    if (type.kind === 'missing' && tryGitSuffix) {
      // Git also accepts a repository name with its trailing .git omitted.
      return withLocalCloneSourceRead(`${path}.git`, operation, false)
    }
    if (type.kind !== 'regular' && type.kind !== 'bare') {
      return operation()
    }
    const resources = await getAssistedCommitProtectedPaths(path)
    const worktrees = await listWorktrees(new Repository(path, -1, null, false))
    const paths = [
      ...new Set(
        await Promise.all(
          [...resources, ...worktrees.map(worktree => worktree.path)].map(
            canonicalMutationPath
          )
        )
      ),
    ]
    // Peer worktrees publish into the same refs and object store that clone reads.
    const read = (index: number): Promise<void> =>
      index === paths.length
        ? operation()
        : withRepositoryGitOperation(paths[index], 'read', () =>
            read(index + 1)
          )
    return read(0)
  })
}

/**
 * Check whether a resolved clone path targets a sensitive location that
 * should never be used as a clone destination. This is a backstop against
 * path traversal attacks where a crafted URL tricks the UI into deriving
 * a clone path outside the intended base directory.
 */
function isClonePathSensitive(unresolvedClonePath: string): boolean {
  const clonePath = Path.resolve(unresolvedClonePath).toLowerCase()
  const home = Path.resolve(homedir()).toLowerCase()

  if (clonePath === home) {
    return true
  }

  const sensitiveLocations = [
    Path.join(home, '.ssh'),
    Path.join(home, '.gnupg'),
    Path.join(home, '.config'),
    Path.join(home, '.config', 'git'),
    Path.join(home, '.gitconfig'),
  ]

  if (__WIN32__) {
    const appData = process.env.APPDATA
    if (appData) {
      sensitiveLocations.push(appData.toLowerCase())
      sensitiveLocations.push(Path.join(appData, 'gnupg').toLowerCase())
    }
  }

  for (const sensitive of sensitiveLocations) {
    if (clonePath === sensitive || clonePath.startsWith(sensitive + Path.sep)) {
      return true
    }
  }

  return false
}

/**
 * Clones a repository from a given url into to the specified path.
 *
 * @param url     - The remote repository URL to clone from
 *
 * @param path    - The destination path for the cloned repository. If the
 *                  path does not exist it will be created. Cloning into an
 *                  existing directory is only allowed if the directory is
 *                  empty.
 *
 * @param options  - Options specific to the clone operation, see the
 *                   documentation for CloneOptions for more details.
 *
 * @param progressCallback - An optional function which will be invoked
 *                           with information about the current progress
 *                           of the clone operation. When provided this enables
 *                           the '--progress' command line flag for
 *                           'git clone'.
 */
export async function clone(
  url: string,
  path: string,
  options: CloneOptions,
  progressCallback?: (progress: ICloneProgress) => void
): Promise<void> {
  const unsupportedProtocol = unsupportedCloneProtocols.find(protocol =>
    url.startsWith(`${protocol}::`)
  )
  if (unsupportedProtocol !== undefined) {
    throw new Error(
      `The "${unsupportedProtocol}" transport is not supported for cloning in ${__APP_NAME__}.`
    )
  }

  if (isClonePathSensitive(path)) {
    throw new Error(
      `The clone destination "${path}" targets a sensitive system location. ` +
        'Cloning into this directory is not allowed.'
    )
  }

  const operationPath = Path.resolve(__dirname, path)
  return withRepositoryGitResourceMutation([operationPath], async () => {
    const destination = await canonicalMutationPath(operationPath)
    return withRepositoryGitResourceMutation([destination], () => {
      const source = getLocalCloneSourcePath(url)
      const operation = () => cloneCore(url, path, options, progressCallback)
      return source === null
        ? operation()
        : withLocalCloneSourceRead(source, operation)
    })
  })
}

function cloneTrampolineEnvironment(
  environment: object
): ICloneTrampolineEnvironment {
  const values = environment as Record<string, unknown>
  const gitSshCommand = values.GIT_SSH_COMMAND
  if (
    typeof values.DESKTOP_PORT !== 'number' ||
    typeof values.DESKTOP_TRAMPOLINE_TOKEN !== 'string' ||
    typeof values.GIT_USER_AGENT !== 'string' ||
    typeof values.SSH_ASKPASS !== 'string' ||
    typeof values.DISPLAY !== 'string' ||
    typeof values.GIT_CONFIG_PARAMETERS !== 'string' ||
    (gitSshCommand !== undefined && typeof gitSshCommand !== 'string')
  ) {
    throw new TypeError('Invalid clone trampoline environment')
  }
  return {
    desktop_port: values.DESKTOP_PORT,
    trampoline_token: values.DESKTOP_TRAMPOLINE_TOKEN,
    user_agent: values.GIT_USER_AGENT,
    ssh_askpass: values.SSH_ASKPASS,
    display: values.DISPLAY,
    ...(gitSshCommand === undefined ? {} : { git_ssh_command: gitSshCommand }),
    config_parameters: values.GIT_CONFIG_PARAMETERS,
  }
}

async function cloneCore(
  url: string,
  path: string,
  options: CloneOptions,
  progressCallback?: (progress: ICloneProgress) => void
): Promise<void> {
  const defaultBranch = options.defaultBranch ?? (await getDefaultBranch())
  const title = `Cloning into ${path}`
  const kind = 'clone'
  const emit = (progress: IGitProgress | IGitOutput) => {
    const description =
      progress.kind === 'progress' ? progress.details.text : progress.text
    withoutRepositoryGitAccess(() =>
      progressCallback?.({ kind, title, description, value: progress.percent })
    )
  }

  const parser =
    progressCallback === undefined
      ? undefined
      : new FetchStreamProgressParser(emit, new CloneProgressParser())
  withoutRepositoryGitAccess(() =>
    progressCallback?.({ kind, title, value: 0 })
  )
  const proxy = backendRemoteProxy(await envForProxy(url))
  try {
    await withTrampolineEnv(
      async trampolineEnvironment =>
        backend().cloneRepository(
          {
            url,
            path,
            default_branch: defaultBranch,
            ...(options.branch === undefined ? {} : { branch: options.branch }),
            ...(proxy === undefined ? {} : { proxy }),
            ...(process.env.GIT_ALLOW_PROTOCOL === undefined
              ? {}
              : { allow_protocol: process.env.GIT_ALLOW_PROTOCOL }),
            trampoline: cloneTrampolineEnvironment(trampolineEnvironment),
          },
          {
            deadlineMs: 3_600_000,
            ...(progressCallback === undefined
              ? {}
              : {
                  onProgress: chunk => {
                    try {
                      parser?.push(chunk)
                    } catch {
                      // Progress observers cannot terminate a clone.
                    }
                  },
                }),
          }
        ),
      __dirname,
      false,
      undefined,
      options.fallbackAccount
    )
  } finally {
    try {
      parser?.finish()
    } catch {
      // Progress observers cannot invalidate a completed clone.
    }
  }
}
