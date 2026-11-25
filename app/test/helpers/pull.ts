import {
  fetch as fetchFromRemote,
  getRemotes,
  pull as pullFromRemote,
} from '../../src/lib/git'
import { Repository } from '../../src/models/repository'
import { IRemote } from '../../src/models/remote'

type PullOptions = Parameters<typeof pullFromRemote>[2]

/**
 * Pull the way the app does: with the URL the remote is configured with. The
 * Rust pull reviews that URL before it runs and refuses a remote whose URL
 * differs, so tests that name a placeholder remote read the real one first.
 */
export async function pull(
  repository: Repository,
  remote: IRemote,
  options?: PullOptions
): Promise<void> {
  const url =
    (await getRemotes(repository)).find(r => r.name === remote.name)?.url ??
    null
  return pullFromRemote(
    repository,
    url === null ? remote : { ...remote, url },
    options
  )
}

/** Fetch with the URL the remote is configured with; see `pull`. */
export async function fetch(
  repository: Repository,
  remote: IRemote
): Promise<void> {
  const url =
    (await getRemotes(repository)).find(r => r.name === remote.name)?.url ??
    null
  return fetchFromRemote(repository, url === null ? remote : { ...remote, url })
}
