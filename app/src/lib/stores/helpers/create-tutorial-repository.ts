import { Account } from '../../../models/account'
import { API } from '../../api'
import { APIError } from '../../http'
import {
  executionOptionsWithProgress,
  PushProgressParser,
} from '../../progress'
import { git } from '../../git'
import { IRemote } from '../../../models/remote'
import { getDefaultBranch } from '../../helpers/default-branch'
import { envForRemoteOperation } from '../../git/environment'
import { pathExists } from '../../path-exists'
import { backend } from '../../../backend/client'

const nl = __WIN32__ ? '\r\n' : '\n'
const InitialReadmeContents =
  `# Welcome to GitHub Desktop!${nl}${nl}` +
  `This is your README. READMEs are where you can communicate ` +
  `what your project is and how to use it.${nl}${nl}` +
  `Write your name on line 6, save it, and then head ` +
  `back to GitHub Desktop.${nl}`

async function createAPIRepository(account: Account, name: string) {
  const api = new API(account.endpoint, account.token)

  try {
    return await api.createRepository(
      null,
      name,
      'GitHub Desktop tutorial repository',
      true
    )
  } catch (err) {
    if (
      err instanceof APIError &&
      err.responseStatus === 422 &&
      err.apiError !== null
    ) {
      if (err.apiError.message === 'Repository creation failed.') {
        if (
          err.apiError.errors &&
          err.apiError.errors.some(
            x => x.message === 'name already exists on this account'
          )
        ) {
          throw new Error(
            'You already have a repository named ' +
              `"${name}" on your account at ${account.friendlyEndpoint}.\n\n` +
              'Please delete the repository and try again.'
          )
        }
      }
    }

    throw err
  }
}

async function pushRepo(
  path: string,
  account: Account,
  remote: IRemote,
  remoteBranchName: string,
  progressCb: (title: string, value: number, description?: string) => void
) {
  const pushTitle = `Pushing repository to ${account.friendlyEndpoint}`
  progressCb(pushTitle, 0)

  const pushOpts = await executionOptionsWithProgress(
    {
      env: await envForRemoteOperation(remote.url),
      // The tutorial repository isn't added to Desktop until after it's been
      // pushed, so its account can't be resolved from its path yet.
      fallbackAccount: { endpoint: account.endpoint, login: account.login },
    },
    new PushProgressParser(),
    progress => {
      if (progress.kind === 'progress') {
        progressCb(pushTitle, progress.percent, progress.details.text)
      }
    }
  )

  const args = ['push', '-u', remote.name, remoteBranchName]
  await git(args, path, 'tutorial:push', pushOpts)
}

/**
 * Creates a repository on the remote (as specified by the Account
 * parameter), initializes an empty repository at the given path,
 * sets up the expected tutorial contents, and pushes the repository
 * to the remote.
 *
 * @param path    The path on the local machine where the tutorial
 *                repository is to be created
 *
 * @param account The account (and thereby the GitHub host) under
 *                which the repository is to be created created
 */
export async function createTutorialRepository(
  account: Account,
  name: string,
  path: string,
  progressCb: (title: string, value: number, description?: string) => void
) {
  progressCb(`Creating repository on ${account.friendlyEndpoint}`, 0)

  if (await pathExists(path)) {
    throw new Error(
      `The path '${path}' already exists. Please move it ` +
        'out of the way, or remove it, and then try again.'
    )
  }

  const repo = await createAPIRepository(account, name)
  const branch = repo.default_branch ?? (await getDefaultBranch())
  progressCb('Initializing local repository', 0.2)

  await backend().initializeDesktopRepository(path, branch)
  await backend().writeRepositoryCreationFile(
    path,
    'readme',
    InitialReadmeContents
  )
  await backend().createInitialCommit(path, ['README.md'])

  const remote: IRemote = { name: 'origin', url: repo.clone_url }
  await backend().manageRepositoryRemote(path, {
    type: 'add',
    name: remote.name,
    url: remote.url,
  })

  await pushRepo(path, account, remote, branch, (title, value, description) => {
    progressCb(title, 0.3 + value * 0.6, description)
  })

  progressCb('Finalizing tutorial repository', 0.9)

  return repo
}
