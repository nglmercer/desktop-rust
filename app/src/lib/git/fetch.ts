import { Repository } from '../../models/repository'
import { IFetchProgress } from '../../models/progress'
import { IRemote } from '../../models/remote'
import { ITrackingBranch } from '../../models/branch'
import { backendRemoteProxy, envForProxy } from './environment'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import { fetchFailureToGitError } from './fetch-error'

/**
 * Fetch from the given remote using the reviewed Rust backend.
 *
 * Proxy settings explicitly resolved for the renderer are forwarded through
 * the typed operation contract; host-inherited proxy variables remain in the
 * host's Git environment and are not copied across IPC.
 */
export async function fetch(
  repository: Repository,
  remote: IRemote,
  progressCallback?: (progress: IFetchProgress) => void,
  isBackgroundTask = false,
  recurseSubmodules = true
): Promise<void> {
  const title = `Fetching ${remote.name}`
  const parser =
    progressCallback === undefined
      ? undefined
      : new FetchStreamProgressParser(progress => {
          if (
            progress.kind === 'context' &&
            !progress.text.startsWith('remote: Counting objects')
          ) {
            return
          }
          progressCallback({
            kind: 'fetch',
            title,
            description:
              progress.kind === 'progress'
                ? progress.details.text
                : progress.text,
            value: progress.percent,
            remote: remote.name,
          })
        })
  progressCallback?.({ kind: 'fetch', title, value: 0, remote: remote.name })
  const proxy = backendRemoteProxy(await envForProxy(remote.url))
  try {
    const failure = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().fetchRepositoryRemote(
          repository.path,
          remote.name,
          remote.url,
          recurseSubmodules,
          isBackgroundTask,
          repository.gitHubRepository?.owner.login,
          parser === undefined ? undefined : chunk => parser.push(chunk),
          undefined,
          proxy
        ),
      true
    )
    if (failure !== undefined) {
      throw fetchFailureToGitError(failure, repository.path, remote.name)
    }
  } finally {
    parser?.finish()
  }
}

/** Fetch a given refspec from the given remote using the reviewed Rust backend. */
export async function fetchRefspec(
  repository: Repository,
  remote: IRemote,
  refspec: string
): Promise<void> {
  const proxy = backendRemoteProxy(await envForProxy(remote.url))
  const failure = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().fetchRepositoryRemote(
        repository.path,
        remote.name,
        remote.url,
        true,
        false,
        repository.gitHubRepository?.owner.login,
        undefined,
        refspec,
        proxy
      ),
    true
  )
  if (failure !== undefined && failure.exitCode !== 128) {
    throw fetchFailureToGitError(failure, repository.path, remote.name)
  }
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

  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().fastForwardRepositoryBranches(
        repository.path,
        branches,
        recurseSubmodules
      ),
    true
  )
}
