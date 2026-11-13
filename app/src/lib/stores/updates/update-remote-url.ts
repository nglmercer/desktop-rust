import { IAPIRepository } from '../../api'
import { GitStore } from '../git-store'
import { urlMatchesRemote } from '../../repository-matching'
import { GitHubRepository } from '../../../models/github-repository'

export async function updateRemoteUrl(
  gitStore: GitStore,
  gitHubRepository: GitHubRepository,
  apiRepo: IAPIRepository
): Promise<void> {
  // I'm not sure when these early exit conditions would be met. But when they are
  // we don't have enough information to continue so exit early!
  if (gitStore.defaultRemote === null) {
    return
  }

  const remoteUrl = gitStore.defaultRemote.url
  const updatedRemoteUrl = apiRepo.clone_url
  const urlsMatch = urlMatchesRemote(updatedRemoteUrl, gitStore.defaultRemote)

  // SCP-style and malformed URLs are user-configured and cannot be safely
  // compared using the standard URL parser.
  let parsedRemoteUrl: URL
  let parsedUpdatedRemoteUrl: URL
  try {
    parsedRemoteUrl = new URL(remoteUrl)
    parsedUpdatedRemoteUrl = new URL(updatedRemoteUrl)
  } catch {
    return
  }
  // An API clone URL does not contain locally configured credentials or URL
  // parameters. Preserve even empty userinfo, query, and fragment delimiters.
  // Backslashes have ambiguous URL/path interpretation, so do not rewrite them.
  if (
    remoteUrl.includes('\\') ||
    /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[^/?#]*@/.test(remoteUrl) ||
    parsedRemoteUrl.username !== '' ||
    parsedRemoteUrl.password !== '' ||
    parsedRemoteUrl.href.includes('?') ||
    parsedRemoteUrl.href.includes('#')
  ) {
    return
  }
  const protocolsMatch =
    parsedRemoteUrl.protocol === parsedUpdatedRemoteUrl.protocol

  // Check if the default remote url has been manually changed from the
  // clone url retrieved from the GitHub API previously
  const remoteUrlUnchanged =
    gitStore.defaultRemote &&
    urlMatchesRemote(gitHubRepository.cloneURL, gitStore.defaultRemote)

  if (protocolsMatch && remoteUrlUnchanged && !urlsMatch) {
    await gitStore.setRemoteURL(gitStore.defaultRemote.name, updatedRemoteUrl)
  }
}
