import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'

let lfsAvailable: Promise<boolean> | undefined

/**
 * Whether the `git lfs` command exists. The bundled Git always shipped with
 * Git LFS; a system Git may not have it installed, in which case `git lfs`
 * is an unknown command. Checked once per run.
 */
export function isLFSAvailable(): Promise<boolean> {
  lfsAvailable ??= backend()
    .lfsAvailable()
    .catch(error => {
      lfsAvailable = undefined
      throw error
    })
  return lfsAvailable
}

/** Install the global LFS filters. */
export async function installGlobalLFSFilters(force: boolean): Promise<void> {
  if (!(await isLFSAvailable())) {
    log.warn('Git LFS is not installed, skipping global LFS filter setup')
    return
  }

  await backend().installGlobalLfsFilters(force)
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

  await backend().installRepositoryLfsHooks(repository.path, force)
}

/** Is the repository configured to track any paths with LFS? */
export async function isUsingLFS(repository: Repository): Promise<boolean> {
  if (!(await isLFSAvailable())) {
    return false
  }

  return backend().repositoryUsesLfs(repository.path)
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
  const result = await backend().repositoryLfsFileTracking(repository.path, [
    path,
  ])
  return result[0]
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
  const untracked: string[] = []
  for (let offset = 0; offset < filePaths.length; offset += 1000) {
    const paths = filePaths.slice(offset, offset + 1000)
    const tracked = await backend().repositoryLfsFileTracking(
      repository.path,
      paths
    )
    paths.forEach((path, index) => {
      if (!tracked[index]) untracked.push(path)
    })
  }
  return untracked
}
