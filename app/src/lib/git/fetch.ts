import { git, IGitStringExecutionOptions } from './core'
import { Repository } from '../../models/repository'
import { IFetchProgress } from '../../models/progress'
import { FetchProgressParser, executionOptionsWithProgress } from '../progress'
import { IRemote } from '../../models/remote'
import { ITrackingBranch } from '../../models/branch'
import { envForProxy, envForRemoteOperation } from './environment'
import { backend, FetchPreviewUnsupported } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import { fetchFailureToGitError } from './fetch-error'

/** Select the native route before mutation, preserving proxy and older-host behavior. */
async function canFetchNatively(
  remoteUrl: string,
  needsProgress: boolean
): Promise<boolean> {
  if (typeof window === 'undefined') {
    return false
  }
  const transport = backend().transportKind
  if (transport === 'ws') {
    return true
  }
  if (transport !== 'wry') {
    return false
  }
  if (needsProgress && typeof (window as any).__desktop?.invokeWithProgress !== 'function') {
    return false
  }
  if (['http_proxy', 'https_proxy', 'HTTPS_PROXY', 'all_proxy', 'ALL_PROXY'].some(key => key in process.env)) {
    return false
  }
  return (await envForProxy(remoteUrl)) === undefined
}

async function getFetchArgs(
  remote: string,
  progressCallback: ((progress: IFetchProgress) => void) | undefined,
  recurseSubmodules: boolean
) {
  return [
    'fetch',
    ...(progressCallback ? ['--progress'] : []),
    '--prune',
    `--recurse-submodules=${recurseSubmodules ? 'on-demand' : 'no'}`,
    '--',
    remote,
  ]
}

/**
 * Fetch from the given remote.
 *
 * @param repository - The repository to fetch into
 *
 * @param account    - The account to use when authenticating with the remote
 *
 * @param remote     - The remote to fetch from
 *
 * @param progressCallback - An optional function which will be invoked
 *                           with information about the current progress
 *                           of the fetch operation. When provided this enables
 *                           the '--progress' command line flag for
 *                           'git fetch'.
 * @param isBackgroundTask  - Whether the fetch is being performed as a
 *                            background task as opposed to being user initiated
 * @param recurseSubmodules - Whether to fetch populated submodules on demand.
 */
export async function fetch(
  repository: Repository,
  remote: IRemote,
  progressCallback?: (progress: IFetchProgress) => void,
  isBackgroundTask = false,
  recurseSubmodules = true
): Promise<void> {
  // Wry falls back only before a mutation when the reviewed route cannot
  // preserve the existing proxy or progress behavior.
  if (await canFetchNatively(remote.url, progressCallback !== undefined)) {
    const title = `Fetching ${remote.name}`
    const parser = progressCallback === undefined ? undefined : new FetchStreamProgressParser(progress => {
      if (progress.kind === 'context' && !progress.text.startsWith('remote: Counting objects')) {
        return
      }
      progressCallback({
        kind: 'fetch',
        title,
        description: progress.kind === 'progress' ? progress.details.text : progress.text,
        value: progress.percent,
        remote: remote.name,
      })
    })
    progressCallback?.({ kind: 'fetch', title, value: 0, remote: remote.name })
    try {
      const failure = await withRepositoryGitOperation(repository.path, 'mutation', () =>
        backend().fetchRepositoryRemote(
          repository.path,
          remote.name,
          remote.url,
          recurseSubmodules,
          isBackgroundTask,
          repository.gitHubRepository?.owner.login,
          parser === undefined ? undefined : chunk => parser.push(chunk)
        ), true)
      if (failure !== undefined) {
        throw fetchFailureToGitError(failure, repository.path, remote.name)
      }
      return
    } catch (error) {
      if (backend().transportKind !== 'wry' || !(error instanceof FetchPreviewUnsupported)) {
        throw error
      }
    } finally {
      parser?.finish()
    }
  }
  let opts: IGitStringExecutionOptions = {
    successExitCodes: new Set([0]),
    env: await envForRemoteOperation(remote.url),
  }

  if (progressCallback) {
    const title = `Fetching ${remote.name}`
    const kind = 'fetch'

    opts = await executionOptionsWithProgress(
      { ...opts, trackLFSProgress: true, isBackgroundTask },
      new FetchProgressParser(),
      progress => {
        // In addition to progress output from the remote end and from
        // git itself, the stderr output from pull contains information
        // about ref updates. We don't need to bring those into the progress
        // stream so we'll just punt on anything we don't know about for now.
        if (progress.kind === 'context') {
          if (!progress.text.startsWith('remote: Counting objects')) {
            return
          }
        }

        const description =
          progress.kind === 'progress' ? progress.details.text : progress.text
        const value = progress.percent

        progressCallback({
          kind,
          title,
          description,
          value,
          remote: remote.name,
        })
      }
    )

    // Initial progress
    progressCallback({ kind, title, value: 0, remote: remote.name })
  }

  const args = await getFetchArgs(
    remote.name,
    progressCallback,
    recurseSubmodules
  )

  await git(args, repository.path, 'fetch', opts)
}

/** Fetch a given refspec from the given remote. */
export async function fetchRefspec(
  repository: Repository,
  remote: IRemote,
  refspec: string
): Promise<void> {
  // The legacy Git path injects an OS/PAC proxy that the native transport
  // cannot yet receive. Choose it before starting a native mutation.
  if (await canFetchNatively(remote.url, false)) {
    try {
      const failure = await withRepositoryGitOperation(repository.path, 'mutation', () =>
        backend().fetchRepositoryRemote(repository.path, remote.name, remote.url, true, false, repository.gitHubRepository?.owner.login, undefined, refspec), true)
      if (failure !== undefined && failure.exitCode !== 128) {
        throw fetchFailureToGitError(failure, repository.path, remote.name)
      }
      return
    } catch (error) {
      if (backend().transportKind !== 'wry' || !(error instanceof FetchPreviewUnsupported)) throw error
    }
  }
  await git(
    ['fetch', '--', remote.name, refspec],
    repository.path,
    'fetchRefspec',
    {
      successExitCodes: new Set([0, 128]),
      env: await envForRemoteOperation(remote.url),
    }
  )
}

/** Fast-forward tracking branches, optionally excluding unadmitted submodules. */
export async function fastForwardBranches(
  repository: Repository,
  branches: ReadonlyArray<ITrackingBranch>,
  recurseSubmodules = true
): Promise<void> {
  if (branches.length === 0) {
    return
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await withRepositoryGitOperation(repository.path, 'mutation', () => backend().fastForwardRepositoryBranches(repository.path, branches, recurseSubmodules), true)
    return
  }

  const refPairs = branches.map(branch => `${branch.upstreamRef}:${branch.ref}`)

  await git(
    [
      'fetch',
      '.',
      // Make sure we don't try to update branches that can't be fast-forwarded
      // even if the user disabled this via the git config option
      // `fetch.showForcedUpdates`
      '--show-forced-updates',
      // Prevent `git fetch` from touching the `FETCH_HEAD`
      '--no-write-fetch-head',
      ...(recurseSubmodules ? [] : ['--recurse-submodules=no']),
      // Take branch refs from stdin to circumvent shell max line length
      // limitations (mainly on Windows)
      '--stdin',
    ],
    repository.path,
    'fastForwardBranches',
    {
      // Fetch exits with an exit code of 1 if one or more refs failed to update
      // which is what we expect will happen
      successExitCodes: new Set([0, 1]),
      env: {
        // This will make sure the reflog entries are correct after
        // fast-forwarding the branches.
        GIT_REFLOG_ACTION: 'pull',
      },
      stdin: refPairs.join('\n'),
    }
  )
}
