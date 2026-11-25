import { Repository } from '../../models/repository'
import { IRemote } from '../../models/remote'
import { getSymbolicRef } from './refs'
import { backend } from '../../backend/client'
import { resolve } from 'path'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * List the remotes, sorted alphabetically by `name`, for a repository.
 *
 * The reviewed backend returns exact URLs only when they can be represented
 * without exposing credentials or ambiguous remote syntax through IPC.
 */
export async function getRemotes(
  repository: Repository
): Promise<ReadonlyArray<IRemote>> {
  const remotes = await withRepositoryGitOperation(
    repository.path,
    'read',
    () => backend().repositorySafeFetchRemotes(resolve(repository.path)),
    true
  )
  if (remotes === null) {
    throw new Error(
      'Remote configuration cannot be safely represented by the native backend'
    )
  }
  return remotes
}

/** Add a new remote with the given URL. */
export async function addRemote(
  repository: Repository,
  name: string,
  url: string
): Promise<IRemote> {
  await manageRemote(repository, { type: 'add', name, url })
  return { url, name }
}

/** Removes an existing remote, or silently errors if it doesn't exist */
export async function removeRemote(
  repository: Repository,
  name: string
): Promise<void> {
  await manageRemote(repository, { type: 'remove', name })
}

/** Changes the URL for the remote that matches the given name  */
export async function setRemoteURL(
  repository: Repository,
  name: string,
  url: string
): Promise<true> {
  await manageRemote(repository, { type: 'set_url', name, url })
  return true
}

async function manageRemote(
  repository: Repository,
  action:
    | {
        readonly type: 'add' | 'set_url'
        readonly name: string
        readonly url: string
      }
    | { readonly type: 'remove'; readonly name: string }
): Promise<void> {
  const result = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().manageRepositoryRemote(resolve(repository.path), action),
    true
  )
  if (result !== 'updated') {
    throw new Error(
      'Remote configuration is unsupported by the reviewed native backend'
    )
  }
}

/**
 * Update the HEAD ref of the remote, which is the default branch.
 *
 * @param isBackgroundTask Whether the fetch is being performed as a
 *                         background task as opposed to being user initiated
 */
export async function updateRemoteHEAD(
  repository: Repository,
  remote: IRemote,
  isBackgroundTask: boolean
): Promise<void> {
  try {
    const result = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().refreshRepositoryRemoteHead(
          repository.path,
          remote.name,
          remote.url,
          isBackgroundTask
        ),
      true
    )
    if (result === 'unsupported') {
      throw new Error('Remote HEAD cannot be reviewed by the native backend')
    }
  } catch (error) {
    // A remote removed since the caller read it has no HEAD to refresh; the
    // previous operation treated that as success too.
    if (
      error instanceof Error &&
      error.message === 'Configured remote no longer exists'
    ) {
      return
    }
    throw error
  }
}

export async function getRemoteHEAD(
  repository: Repository,
  remote: string
): Promise<string | null> {
  const remoteNamespace = `refs/remotes/${remote}/`
  const match = await getSymbolicRef(repository, `${remoteNamespace}HEAD`)
  if (
    match != null &&
    match.length > remoteNamespace.length &&
    match.startsWith(remoteNamespace)
  ) {
    // strip out everything related to the remote because this
    // is likely to be a tracked branch locally
    return match.substring(remoteNamespace.length)
  }

  return null
}
