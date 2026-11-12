import { git } from './core'
import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'

interface ILFSTrackOutput {
  readonly patterns: ReadonlyArray<{
    readonly tracked: boolean
  }>
}

let lfsAvailable: Promise<boolean> | undefined

/**
 * Whether the `git lfs` command exists. The bundled Git always shipped with
 * Git LFS; a system Git may not have it installed, in which case `git lfs`
 * is an unknown command. Checked once per run.
 */
export function isLFSAvailable(): Promise<boolean> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    lfsAvailable ??= backend().lfsAvailable().catch(error => {
      lfsAvailable = undefined
      throw error
    })
    return lfsAvailable
  }
  lfsAvailable ??= git(['lfs', 'version'], __dirname, 'isLFSAvailable', {
    successExitCodes: new Set([0, 1, 127]),
  }).then(
    result => result.exitCode === 0,
    () => false
  )
  return lfsAvailable
}

/** Install the global LFS filters. */
export async function installGlobalLFSFilters(force: boolean): Promise<void> {
  if (!(await isLFSAvailable())) {
    log.warn('Git LFS is not installed, skipping global LFS filter setup')
    return
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await backend().installGlobalLfsFilters(force)
    return
  }

  const args = ['lfs', 'install', '--skip-repo']
  if (force) {
    args.push('--force')
  }

  await git(args, __dirname, 'installGlobalLFSFilter')
}

/** Install LFS hooks in the repository. */
export async function installLFSHooks(
  repository: Repository,
  force: boolean
): Promise<void> {
  if (!(await isLFSAvailable())) {
    log.warn('Git LFS is not installed, skipping LFS hook setup')
    return
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await backend().installRepositoryLfsHooks(repository.path, force)
    return
  }

  const args = ['lfs', 'install']
  if (force) {
    args.push('--force')
  }

  await git(args, repository.path, 'installLFSHooks')
}

/** Is the repository configured to track any paths with LFS? */
export async function isUsingLFS(repository: Repository): Promise<boolean> {
  if (!(await isLFSAvailable())) {
    return false
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    return backend().repositoryUsesLfs(repository.path)
  }

  const env = {
    GIT_LFS_TRACK_NO_INSTALL_HOOKS: '1',
  }
  const result = await git(
    ['lfs', 'track', '--json'],
    repository.path,
    'isUsingLFS',
    {
      env,
    }
  )

  try {
    const output = JSON.parse(result.stdout) as Partial<ILFSTrackOutput>

    if (!Array.isArray(output.patterns)) {
      return false
    }

    return output.patterns.some(pattern => pattern.tracked)
  } catch {
    return false
  }
}

/**
 * Check if a provided file path is being tracked by Git LFS
 *
 * This uses the Git plumbing to read the .gitattributes file
 * for any LFS-related rules that are set for the file
 *
 * @param repository repository with
 * @param path relative file path in the repository
 */
export async function isTrackedByLFS(
  repository: Repository,
  path: string
): Promise<boolean> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    const result = await backend().repositoryLfsFileTracking(repository.path, [path])
    return result[0]
  }
  const { stdout } = await git(
    ['check-attr', 'filter', path],
    repository.path,
    'checkAttrForLFS'
  )

  // "git check-attr -a" will output every filter it can find in .gitattributes
  // and it looks like this:
  //
  // README.md: diff: lfs
  // README.md: merge: lfs
  // README.md: text: unset
  // README.md: filter: lfs
  //
  // To verify git-lfs this test will just focus on that last row, "filter",
  // and the value associated with it. If nothing is found in .gitattributes
  // the output will look like this
  //
  // README.md: filter: unspecified

  const lfsFilterRegex = /: filter: lfs/

  const match = lfsFilterRegex.exec(stdout)

  return match !== null
}

/**
 * Query a Git repository and filter the set of provided relative paths to see
 * which are not covered by the current Git LFS configuration.
 *
 * @param repository
 * @param filePaths List of relative paths in the repository
 */
export async function filesNotTrackedByLFS(
  repository: Repository,
  filePaths: ReadonlyArray<string>
): Promise<ReadonlyArray<string>> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    const untracked: string[] = []
    for (let offset = 0; offset < filePaths.length; offset += 1000) {
      const paths = filePaths.slice(offset, offset + 1000)
      const tracked = await backend().repositoryLfsFileTracking(repository.path, paths)
      paths.forEach((path, index) => {
        if (!tracked[index]) untracked.push(path)
      })
    }
    return untracked
  }
  const filesNotTrackedByGitLFS = new Array<string>()

  for (const file of filePaths) {
    const isTracked = await isTrackedByLFS(repository, file)

    if (!isTracked) {
      filesNotTrackedByGitLFS.push(file)
    }
  }

  return filesNotTrackedByGitLFS
}
