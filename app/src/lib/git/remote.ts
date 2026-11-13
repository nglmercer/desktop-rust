import { git, IGitStringExecutionOptions } from './core'
import { GitError } from 'dugite'

import { Repository } from '../../models/repository'
import { IRemote } from '../../models/remote'
import { envForRemoteOperation } from './environment'
import { getSymbolicRef } from './refs'
import { backend } from '../../backend/client'
import { resolve } from 'path'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * List the remotes, sorted alphabetically by `name`, for a repository.
 *
 * Returns the first fetch URL for each remote, with Git's URL rewrites applied.
 * Remotes without a fetch URL are omitted.
 * Optional environment overrides apply only to these lookups.
 */
export async function getRemotes(
  repository: Repository,
  options?: Pick<IGitStringExecutionOptions, 'env'>
): Promise<ReadonlyArray<IRemote>> {
  if (
    typeof window !== 'undefined' &&
    backend().transportKind !== 'none' &&
    options?.env === undefined
  ) {
    const read = () =>
      backend().repositorySafeFetchRemotes(resolve(repository.path))
    const remotes = await (backend().transportKind === 'wry'
      ? withRepositoryGitOperation(repository.path, 'read', read, true)
      : read())
    if (remotes !== null) return remotes
  }
  // Config queries also work outside repositories, where they can still return
  // global remotes. Preserve the empty result for non-repository directories.
  const result = await git(
    ['rev-parse', '--git-dir'],
    repository.path,
    'getRemotes',
    { ...options, expectedErrors: new Set([GitError.NotAGitRepository]) }
  )

  if (result.gitError === GitError.NotAGitRepository) {
    return []
  }

  // Unlike `remote -v`, this format keeps tabs and line separators in URLs
  // separate from remote names. Don't restrict the query to --local: included
  // and globally configured remotes should remain visible.
  const config = await git(
    ['config', '--null', '--get-regexp', '^remote\\..+\\.url$'],
    repository.path,
    'getRemotes',
    {
      ...options,
      // Git returns 1 when no URL keys match, including in a new repository.
      successExitCodes: new Set([0, 1]),
    }
  )

  const names = new Set<string>()
  // Output is key<LF>value<NUL>. The trailing NUL creates an empty split element.
  // Only the first LF separates the key and value; later LFs belong to the URL.
  for (const entry of config.stdout.split('\0').slice(0, -1)) {
    const separator = entry.indexOf('\n')
    // A valueless setting has no LF, unlike an explicitly empty URL value.
    if (separator === -1) {
      throw new Error('Remote URL configuration is missing a value')
    }

    const name = entry.slice('remote.'.length, separator - '.url'.length)
    if (entry.slice(separator + 1).length === 0) {
      // An empty URL entry clears any URLs previously configured for this remote.
      names.delete(name)
    } else {
      names.add(name)
    }
  }

  const remotes: IRemote[] = []
  for (const name of [...names].sort()) {
    // Let Git select the first fetch URL and apply insteadOf rewrites rather
    // than returning raw config values or implementing those rules ourselves.
    // Unlike remote get-url, this also resolves globally configured remotes.
    // --get-url does not contact the remote.
    // Match `remote -v` enumeration even when a URL contains credentials.
    // Override the transfer policy only for this offline lookup, not transfers.
    const { stdout } = await git(
      [
        '-c',
        'transfer.credentialsInUrl=allow',
        'ls-remote',
        '--get-url',
        '--',
        name,
      ],
      repository.path,
      'getRemotes',
      options
    )
    // Remove only Git's final LF; trimEnd() would also remove URL whitespace.
    remotes.push({ name, url: stdout.slice(0, -1) })
  }

  return remotes
}

/** Add a new remote with the given URL. */
export async function addRemote(
  repository: Repository,
  name: string,
  url: string
): Promise<IRemote> {
  if (await manageRemote(repository, { type: 'add', name, url })) {
    return { url, name }
  }
  await git(['remote', 'add', '--', name, url], repository.path, 'addRemote')

  return { url, name }
}

/** Removes an existing remote, or silently errors if it doesn't exist */
export async function removeRemote(
  repository: Repository,
  name: string
): Promise<void> {
  if (await manageRemote(repository, { type: 'remove', name })) return
  const options = {
    successExitCodes: new Set([0, 2, 128]),
  }

  await git(
    ['remote', 'remove', '--', name],
    repository.path,
    'removeRemote',
    options
  )
}

/** Changes the URL for the remote that matches the given name  */
export async function setRemoteURL(
  repository: Repository,
  name: string,
  url: string
): Promise<true> {
  if (await manageRemote(repository, { type: 'set_url', name, url })) return true
  await git(
    ['remote', 'set-url', '--', name, url],
    repository.path,
    'setRemoteURL'
  )
  return true
}

async function manageRemote(
  repository: Repository,
  action: { readonly type: 'add' | 'set_url'; readonly name: string; readonly url: string } | { readonly type: 'remove'; readonly name: string }
): Promise<boolean> {
  if (typeof window === 'undefined' || backend().transportKind === 'none') return false
  const result = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().manageRepositoryRemote(resolve(repository.path), action),
    true
  )
  return result === 'updated'
}

/**
 * Get the first fetch URL for the remote, with Git's URL rewrites applied.
 *
 * Returns null if the remote could not be found. The returned URL excludes
 * Git's output terminator but preserves whitespace belonging to the URL.
 */
export async function getRemoteURL(
  repository: Repository,
  name: string
): Promise<string | null> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    const read = () =>
      backend().repositorySafeRemoteUrl(resolve(repository.path), name)
    const result = await (backend().transportKind === 'wry'
      ? withRepositoryGitOperation(repository.path, 'read', read, true)
      : read())
    if (result.kind === 'resolved') return result.url
    if (result.kind === 'missing') return null
  }
  const result = await git(
    ['remote', 'get-url', '--', name],
    repository.path,
    'getRemoteURL',
    {
      // 2 means a missing remote; 128 can also mean malformed configuration.
      successExitCodes: new Set([0, 2]),
      expectedErrors: new Set([GitError.NotAGitRepository]),
    }
  )

  if (result.exitCode !== 0) {
    return null
  }

  // The URL itself may end in LF, so strip exactly one output terminator.
  return result.stdout.slice(0, -1)
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
  if (typeof window !== 'undefined' && backend().transportKind === 'ws') {
    await withRepositoryGitOperation(repository.path, 'mutation', () =>
      backend().refreshRepositoryRemoteHead(repository.path, remote.name, remote.url, isBackgroundTask), true)
    return
  }
  const options = {
    successExitCodes: new Set([0, 1, 128]),
    env: await envForRemoteOperation(remote.url),
    isBackgroundTask,
  }

  await git(
    ['remote', 'set-head', '-a', '--', remote.name],
    repository.path,
    'updateRemoteHEAD',
    options
  )
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
    // e.g. `main`, `develop`, etc
    return match.substring(remoteNamespace.length)
  }

  return null
}
