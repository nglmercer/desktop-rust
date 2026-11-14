import {
  git,
  gitRebaseArguments,
  HookProgress,
  IGitStringExecutionOptions,
  TerminalOutput,
  TerminalOutputCallback,
} from './core'
import { Repository } from '../../models/repository'
import { IPullProgress } from '../../models/progress'
import { PullProgressParser, executionOptionsWithProgress } from '../progress'
import { IRemote } from '../../models/remote'
import { envForRemoteOperation } from './environment'
import { getConfigValue } from './config'
import { GitError as DugiteError } from 'dugite'
import { backend, FetchPreviewUnsupported } from '../../backend/client'
import { envForProxy } from './environment'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import {
  withoutRepositoryGitAccess,
  withRepositoryGitOperation,
} from './repository-operation'
import { fetchFailureToGitError } from './fetch-error'
import { getDescriptionForError, GitError } from './core'
import { getHooksEnvEnabled } from '../hooks/config'

/**
 * Pull from the specified remote.
 *
 * @param repository - The repository in which the pull should take place
 *
 * @param remote     - The name of the remote that should be pulled from
 *
 * @param progressCallback - An optional function which will be invoked
 *                           with information about the current progress
 *                           of the pull operation. When provided this enables
 *                           the '--progress' command line flag for
 *                           'git pull'.
 */
export async function pull(
  repository: Repository,
  remote: IRemote,
  options?: {
    progressCallback?: (progress: IPullProgress) => void
    onHookProgress?: (progress: HookProgress) => void
    onHookFailure?: (
      hookName: string,
      terminalOutput: TerminalOutput
    ) => Promise<'abort' | 'ignore'>
    onTerminalOutputAvailable?: TerminalOutputCallback
    noVerify?: boolean
  }
): Promise<void> {
  // Git pull forwards the remote name to its child git fetch without a `--`
  // separator. Reject leading dashes here even though direct fetch and push
  // can handle them with a separator.
  if (remote.name.startsWith('-')) {
    throw new Error("Cannot pull from a remote whose name starts with '-'.")
  }

  // Select a native route only before Git starts. Unsupported hook/submodule
  // handling may use the prior Wry path, but an attempted pull is never retried.
  const transport =
    typeof window === 'undefined' ? 'none' : backend().transportKind
  const wry = transport === 'wry'
  const hasTerminalObserver = options?.onTerminalOutputAvailable !== undefined
  // The legacy runner intercepts hooks and loads a configured shell only when
  // this preference is enabled. Preserve that path until the native hook
  // environment and callbacks have equivalent support.
  const usesHookEnvironment = nativeTransport(transport) && getHooksEnvEnabled()
  let native = transport === 'ws' || wry
  if (wry && native) {
    if (
      usesHookEnvironment ||
      hasTerminalObserver ||
      (options?.progressCallback &&
        typeof (window as any).__desktop?.invokeWithProgress !== 'function')
    ) {
      native = false
    } else if (
      [
        'http_proxy',
        'https_proxy',
        'HTTPS_PROXY',
        'all_proxy',
        'ALL_PROXY',
      ].some(key => key in process.env)
    ) {
      native = false
    } else if ((await envForProxy(remote.url)) !== undefined) {
      native = false
    }
  }
  if (transport === 'ws' && hasTerminalObserver) {
    throw new FetchPreviewUnsupported(
      'Pull terminal output is unavailable through this backend'
    )
  }
  if (transport === 'ws' && usesHookEnvironment) {
    throw new FetchPreviewUnsupported(
      'The configured hook environment is unavailable through this backend'
    )
  }
  if (native) {
    const title = `Pulling ${remote.name}`
    const callback = options?.progressCallback
    const observe = (progress: IPullProgress) => {
      try {
        withoutRepositoryGitAccess(() => callback?.(progress))
      } catch {
        // An observer cannot change the outcome of a running Git operation.
      }
    }
    const parser =
      callback === undefined
        ? undefined
        : new FetchStreamProgressParser(progress => {
            if (
              progress.kind === 'context' &&
              !progress.text.startsWith('remote: Counting objects')
            ) {
              return
            }
            observe({
              kind: 'pull',
              title,
              remote: remote.name,
              value: progress.percent,
              description:
                progress.kind === 'progress'
                  ? progress.details.text
                  : progress.text,
            })
          }, new PullProgressParser())
    if (callback !== undefined) {
      observe({ kind: 'pull', title, value: 0, remote: remote.name })
    }
    try {
      const failure = await withRepositoryGitOperation(
        repository.path,
        'mutation',
        () =>
          backend().pullRepositoryRemote(
            repository.path,
            {
              remote: remote.name,
              expected_url: remote.url,
              no_verify: options?.noVerify ?? false,
              report_progress: parser !== undefined,
              require_hook_callbacks: false,
              ...(repository.gitHubRepository?.owner.login === undefined
                ? {}
                : {
                    expected_saml_owner:
                      repository.gitHubRepository.owner.login,
                  }),
            },
            parser === undefined ? undefined : chunk => parser.push(chunk)
          ),
        true
      )
      if (failure !== undefined) {
        const localErrors = {
          merge_conflicts: [
            DugiteError.MergeConflicts,
            'Resolve the merge conflicts before continuing.',
          ],
          rebase_conflicts: [
            DugiteError.RebaseConflicts,
            'Resolve the rebase conflicts before continuing.',
          ],
          merge_local_changes: [
            DugiteError.MergeWithLocalChanges,
            'Local changes would be overwritten by the merge. Commit or stash them before pulling.',
          ],
          rebase_local_changes: [
            DugiteError.RebaseWithLocalChanges,
            'Commit or stash your local changes before pulling with rebase.',
          ],
          local_changes_overwritten: [
            DugiteError.LocalChangesOverwritten,
            'Local changes or untracked files would be overwritten. Commit, stash, or move them before pulling.',
          ],
          unrelated_histories: [
            DugiteError.CannotMergeUnrelatedHistories,
            'Unable to merge unrelated histories in this repository.',
          ],
          config_lock_exists: [
            DugiteError.ConfigLockFileAlreadyExists,
            'A Git configuration lock already exists. Inspect the lock before retrying.',
          ],
          no_matching_remote_branch: [
            DugiteError.NoMatchingRemoteBranch,
            'No matching branch was fetched from the remote.',
          ],
          no_existing_remote_branch: [
            DugiteError.NoExistingRemoteBranch,
            'The configured upstream branch was not found on the remote.',
          ],
          missing_upstream: [
            null,
            'The current branch has no upstream branch configured.',
          ],
        } as const
        switch (failure.errorKind) {
          case 'merge_conflicts':
          case 'rebase_conflicts':
          case 'merge_local_changes':
          case 'rebase_local_changes':
          case 'local_changes_overwritten':
          case 'unrelated_histories':
          case 'config_lock_exists':
          case 'no_matching_remote_branch':
          case 'no_existing_remote_branch':
          case 'missing_upstream': {
            const [kind, description] = localErrors[failure.errorKind]
            throw new GitError(
              {
                exitCode: failure.exitCode,
                stdout: '',
                stderr: '',
                path: repository.path,
                gitError: kind,
                gitErrorDescription:
                  kind === null
                    ? description
                    : getDescriptionForError(kind, '') ?? description,
              },
              ['pull', '--', remote.name],
              ''
            )
          }
        }
        throw fetchFailureToGitError(
          { exitCode: failure.exitCode, errorKind: failure.errorKind },
          repository.path,
          remote.name,
          'pull'
        )
      }
      return
    } catch (error) {
      if (!wry || !(error instanceof FetchPreviewUnsupported)) {
        throw error
      }
    } finally {
      parser?.finish()
    }
  }

  let opts: IGitStringExecutionOptions = {
    env: await envForRemoteOperation(remote.url),
    // git pull triggers merge or rebase hooks depending on config, instead of
    // trying to check pull.rebase and friends we'll just intercept all possible
    // hooks that could be run as part of a pull operation.
    interceptHooks: [
      'pre-merge-commit',
      'prepare-commit-msg',
      'commit-msg',
      'post-merge',
      'pre-rebase',
      'pre-commit',
      'post-rewrite',
    ],
    onHookProgress: options?.onHookProgress,
    onHookFailure: options?.onHookFailure,
    onTerminalOutputAvailable: options?.onTerminalOutputAvailable,
  }

  if (options?.progressCallback) {
    const title = `Pulling ${remote.name}`
    const kind = 'pull'

    opts = await executionOptionsWithProgress(
      { ...opts, trackLFSProgress: true },
      new PullProgressParser(),
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

        options?.progressCallback?.({
          kind,
          title,
          description,
          value,
          remote: remote.name,
        })
      }
    )

    // Initial progress
    options.progressCallback({ kind, title, value: 0, remote: remote.name })
  }

  const args = [
    ...gitRebaseArguments(),
    'pull',
    ...(await getDefaultPullDivergentBranchArguments(repository)),
    '--recurse-submodules',
    ...(options?.progressCallback ? ['--progress'] : []),
    ...(options?.noVerify ? ['--no-verify'] : []),
    '--',
    remote.name,
  ]

  await git(args, repository.path, 'pull', opts)
}

function nativeTransport(transport: string): boolean {
  return transport === 'wry' || transport === 'ws'
}

/**
 * Defaults the pull default for divergent paths to try to fast forward and if
 * not perform a merge. Aka uses the flag --ff
 *
 * It checks whether the user has a config set for this already, if so, no need for
 * default.
 */
async function getDefaultPullDivergentBranchArguments(
  repository: Repository
): Promise<ReadonlyArray<string>> {
  try {
    const pullFF = await getConfigValue(repository, 'pull.ff')
    return pullFF !== null ? [] : ['--ff']
  } catch (e) {
    log.error("Couldn't read 'pull.ff' config", e)
  }

  // If there is a failure in checking the config, we still want to use any
  // config and not overwrite the user's set config behavior. This will show the
  // git error if no config is set.
  return []
}
