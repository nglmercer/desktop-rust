import { HookProgress, TerminalOutput, TerminalOutputCallback } from './core'
import { Repository } from '../../models/repository'
import { IPullProgress } from '../../models/progress'
import { PullProgressParser } from '../progress'
import { IRemote } from '../../models/remote'
import { backendRemoteProxy, envForProxy } from './environment'
import { GitErrorKind } from './error-kind'
import { backend } from '../../backend/client'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import {
  withoutRepositoryGitAccess,
  withRepositoryGitOperation,
} from './repository-operation'
import { fetchFailureToGitError } from './fetch-error'
import { pushTerminalChunk } from './push-terminal-chunk'
import { getDescriptionForError, GitError } from './core'
import { rendererHookEnvironment, withHooksEnv } from '../hooks/with-hooks-env'

const pullInterceptHooks = [
  'pre-merge-commit',
  'prepare-commit-msg',
  'commit-msg',
  'post-merge',
  'pre-rebase',
  'pre-commit',
  'post-rewrite',
]

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
  const terminalObserver = options?.onTerminalOutputAvailable
  const needsStreaming = parser !== undefined || terminalObserver !== undefined
  const listeners = new Set<(chunk: TerminalOutput) => void>()
  const terminalChunks: string[] = []
  const notifyTerminal = (text: string) => {
    if (!text) {
      return
    }
    pushTerminalChunk(terminalChunks, 256 * 1024, text)
    for (const listener of listeners) {
      try {
        withoutRepositoryGitAccess(() => listener(text))
      } catch {
        // Observers cannot terminate a pull.
      }
    }
  }
  const registerTerminalObserver = () => {
    if (terminalObserver === undefined) {
      return
    }
    try {
      withoutRepositoryGitAccess(() =>
        terminalObserver(listener => {
          for (const chunk of terminalChunks) {
            try {
              withoutRepositoryGitAccess(() => listener(chunk))
            } catch {
              // Observers cannot terminate a pull.
            }
          }
          listeners.add(listener)
          return {
            unsubscribe: () => listeners.delete(listener),
          }
        })
      )
    } catch {
      // Observer setup must not terminate a pull.
    }
  }
  const decoders = {
    stdout: new TextDecoder(),
    stderr: new TextDecoder(),
  }
  const proxy = backendRemoteProxy(await envForProxy(remote.url))
  try {
    const failure = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        withHooksEnv(
          hookEnvironment =>
            backend().pullRepositoryRemote(
              repository.path,
              {
                remote: remote.name,
                expected_url: remote.url,
                no_verify: options?.noVerify ?? false,
                report_progress: needsStreaming,
                require_hook_callbacks: false,
                ...(repository.gitHubRepository?.owner.login === undefined
                  ? {}
                  : {
                      expected_saml_owner:
                        repository.gitHubRepository.owner.login,
                    }),
                ...(proxy === undefined ? {} : { proxy }),
                ...(rendererHookEnvironment(hookEnvironment) === undefined
                  ? {}
                  : {
                      hook_environment:
                        rendererHookEnvironment(hookEnvironment),
                    }),
              },
              !needsStreaming
                ? undefined
                : chunk => {
                    try {
                      parser?.push(chunk)
                    } catch {
                      // Progress observers cannot terminate a pull.
                    }
                    if (chunk.stream === 'lfs') {
                      return
                    }
                    const bytes = Uint8Array.from(atob(chunk.data), character =>
                      character.charCodeAt(0)
                    )
                    notifyTerminal(
                      decoders[chunk.stream].decode(bytes, { stream: true })
                    )
                  },
              terminalObserver === undefined
                ? undefined
                : registerTerminalObserver
            ),
          repository.path,
          {
            env: {},
            interceptHooks: pullInterceptHooks,
            onHookProgress: options?.onHookProgress,
            onHookFailure: options?.onHookFailure,
          }
        ),
      true
    )
    if (failure !== undefined) {
      const localErrors = {
        merge_conflicts: [
          GitErrorKind.MergeConflicts,
          'Resolve the merge conflicts before continuing.',
        ],
        rebase_conflicts: [
          GitErrorKind.RebaseConflicts,
          'Resolve the rebase conflicts before continuing.',
        ],
        merge_local_changes: [
          GitErrorKind.MergeWithLocalChanges,
          'Local changes would be overwritten by the merge. Commit or stash them before pulling.',
        ],
        rebase_local_changes: [
          GitErrorKind.RebaseWithLocalChanges,
          'Commit or stash your local changes before pulling with rebase.',
        ],
        local_changes_overwritten: [
          GitErrorKind.LocalChangesOverwritten,
          'Local changes or untracked files would be overwritten. Commit, stash, or move them before pulling.',
        ],
        unrelated_histories: [
          GitErrorKind.CannotMergeUnrelatedHistories,
          'Unable to merge unrelated histories in this repository.',
        ],
        config_lock_exists: [
          GitErrorKind.ConfigLockFileAlreadyExists,
          'A Git configuration lock already exists. Inspect the lock before retrying.',
        ],
        no_matching_remote_branch: [
          GitErrorKind.NoMatchingRemoteBranch,
          'No matching branch was fetched from the remote.',
        ],
        no_existing_remote_branch: [
          GitErrorKind.NoExistingRemoteBranch,
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
                  : (getDescriptionForError(kind, '') ?? description),
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
  } finally {
    parser?.finish()
  }
}
