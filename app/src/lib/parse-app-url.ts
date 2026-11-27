import { testForInvalidChars } from './sanitize-ref-name'

export interface IOAuthAction {
  readonly name: 'oauth'
  readonly code: string
  readonly state: string
}

export interface IOpenRepositoryFromURLAction {
  readonly name: 'open-repository-from-url'

  /** the remote repository location associated with the "Open in Desktop" action */
  readonly url: string

  /** the optional branch name which should be checked out. use the default branch otherwise. */
  readonly branch: string | null

  /** the pull request number, if pull request originates from a fork of the repository */
  readonly pr: string | null

  /** the file to open after cloning the repository */
  readonly filepath: string | null
}

export interface IUnknownAction {
  readonly name: 'unknown'
  readonly url: string
}

export type URLActionType =
  | IOAuthAction
  | IOpenRepositoryFromURLAction
  | IUnknownAction

/**
 * Parse the URL to find a given key in the querystring text.
 *
 * @param url The source URL containing querystring key-value pairs
 * @param key The key to look for in the querystring
 */
function getQueryStringValue(
  query: URLSearchParams,
  key: string
): string | null {
  return query.get(key)
}

export function parseAppURL(url: string): URLActionType {
  const unknown: IUnknownAction = { name: 'unknown', url }
  let parsedURL: URL
  try {
    parsedURL = new URL(url)
  } catch {
    return unknown
  }

  const hostname = parsedURL.hostname
  if (!hostname) {
    return unknown
  }

  const query = parsedURL.searchParams

  const actionName = hostname.toLowerCase()
  if (actionName === 'oauth') {
    const code = getQueryStringValue(query, 'code')
    const state = getQueryStringValue(query, 'state')
    if (code != null && state != null) {
      return { name: 'oauth', code, state }
    } else {
      return unknown
    }
  }

  // we require something resembling a URL first
  // - bail out if it's not defined
  // - bail out if you only have `/`
  const pathName = parsedURL.pathname
  if (!pathName || pathName.length <= 1) {
    return unknown
  }

  // Trim the trailing / from the URL
  const parsedPath = pathName.substring(1)

  if (actionName === 'openrepo') {
    const pr = getQueryStringValue(query, 'pr')
    const branch = getQueryStringValue(query, 'branch')
    const filepath = getQueryStringValue(query, 'filepath')

    if (pr != null) {
      if (!/^\d+$/.test(pr)) {
        return unknown
      }

      // we also expect the branch for a forked PR to be a given ref format
      if (branch != null && !/^pr\/\d+$/.test(branch)) {
        return unknown
      }
    }

    if (branch != null && testForInvalidChars(branch)) {
      return unknown
    }

    return {
      name: 'open-repository-from-url',
      url: parsedPath,
      branch,
      pr,
      filepath,
    }
  }

  return unknown
}
