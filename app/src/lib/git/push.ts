import { GitError, HookCallbackOptions, TerminalOutput } from './core'
import { Repository } from '../../models/repository'
import { IPushProgress } from '../../models/progress'
import { PushProgressParser } from '../progress'
import { IRemote } from '../../models/remote'
import { backendRemoteProxy, envForProxy } from './environment'
import { Branch } from '../../models/branch'
import {
  backend,
  BackendError,
  FetchErrorKind,
  PushErrorKind,
  type ICloneTrampolineEnvironment,
} from '../../backend/client'
import {
  withRepositoryGitOperation,
  withoutRepositoryGitAccess,
} from './repository-operation'
import { FetchStreamProgressParser } from '../progress/fetch-stream'
import { fetchFailureToGitError } from './fetch-error'
import { pushTerminalChunk } from './push-terminal-chunk'
import { GitErrorKind } from './error-kind'
import { rendererHookEnvironment, withHooksEnv } from '../hooks/with-hooks-env'

const uncertainPushes = new WeakSet<Error>()

function isPushErrorKind(
  kind: FetchErrorKind | PushErrorKind | null
): kind is PushErrorKind {
  return kind !== null && kind.startsWith('push_')
}

/** A native push may have changed a destination; do not offer a blind retry. */
export function isNativePushOutcomeUncertain(error: unknown): boolean {
  return error instanceof Error && uncertainPushes.has(error)
}

/** Remove the retry action once a push may have reached the remote. */
export function suppressUnsafePushRetry(
  metadata: { retryAction?: unknown },
  error: unknown,
  pushed: boolean
): void {
  if (pushed || isNativePushOutcomeUncertain(error)) delete metadata.retryAction
}

const pushErrors: Record<
  Exclude<PushErrorKind, 'push_protection'>,
  GitErrorKind
> = {
  push_non_fast_forward: GitErrorKind.PushNotFastForward,
  push_file_too_large: GitErrorKind.PushWithFileSizeExceedingLimit,
  push_private_email: GitErrorKind.PushWithPrivateEmail,
  push_review_required: GitErrorKind.ProtectedBranchRequiresReview,
  push_protected_force: GitErrorKind.ProtectedBranchForcePush,
  push_required_status: GitErrorKind.ProtectedBranchRequiredStatus,
  push_force_rejected: GitErrorKind.ForcePushRejected,
}

/** Preserve native pushInsteadOf behavior when push falls back to the fetch URL. */
export type AssistedCommitPushURLSource = 'push-url' | 'fetch-url'

export type PushOptions = {
  /**
   * Force-push the branch without losing changes in the remote that
   * haven't been fetched.
   *
   * See https://git-scm.com/docs/git-push#Documentation/git-push.txt---no-force-with-lease
   */
  readonly forceWithLease?: boolean

  /** A branch to push instead of the current branch */
  readonly branch?: Branch

  readonly noVerify?: boolean

  /** Renderer trampoline credential routing for repositories without an assigned account. */
  readonly trampoline?: ICloneTrampolineEnvironment

  /** Desktop-owned assisted entry; never enables force, tag, or submodule pushes. */
  readonly assistedCommit?: {
    readonly expectedTip: string
    readonly remoteRef: string
    readonly pushURL: string
    readonly rawPushURL: string
    readonly pushURLSource: AssistedCommitPushURLSource
    readonly prepareForSpawn: () => Promise<() => void>
  }
} & HookCallbackOptions

/** Pin the original native URL collection without logging URL credentials. */
export function getAssistedCommitPushConfigParameters(
  remoteName: string,
  rawURL: string,
  source: AssistedCommitPushURLSource
): string {
  const parameters = process.env.GIT_CONFIG_PARAMETERS ?? ''
  const property = source === 'push-url' ? 'pushurl' : 'url'
  return `${parameters}${parameters.length === 0 ? '' : ' '}${[
    `remote.${remoteName}.${property}=`,
    `remote.${remoteName}.${property}=${rawURL}`,
    `remote.${remoteName}.mirror=false`,
  ]
    .map(parameter => `'${parameter.replace(/'/g, "'\\''")}'`)
    .join(' ')}`
}

/**
 * Push from the remote to the branch, optionally setting the upstream.
 *
 * @param repository - The repository from which to push
 *
 * @param account - The account to use when authenticating with the remote
 *
 * @param remote - The remote to push the specified branch to
 *
 * @param localBranch - The local branch to push
 *
 * @param remoteBranch - The remote branch to push to
 *
 * @param tagsToPush - The tags to push along with the branch.
 *
 * @param options - Optional customizations for the push execution.
 *                  see PushOptions for more information.
 *
 * @param progressCallback - An optional function which will be invoked
 *                           with information about the current progress
 *                           of the push operation. When provided this enables
 *                           the '--progress' command line flag for
 *                           'git push'.
 */
export async function push(
  repository: Repository,
  remote: IRemote,
  localBranch: string,
  remoteBranch: string | null,
  tagsToPush: ReadonlyArray<string> | null,
  options?: PushOptions,
  progressCallback?: (progress: IPushProgress) => void
): Promise<void> {
  const assisted = options?.assistedCommit
  if (
    assisted !== undefined &&
    (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(assisted.expectedTip) ||
      !assisted.remoteRef.startsWith('refs/heads/') ||
      options?.forceWithLease ||
      options?.noVerify)
  ) {
    throw new Error(
      'Assisted push requires a full commit ID and a normal branch push'
    )
  }
  const assistedConfigParameters =
    assisted === undefined
      ? undefined
      : getAssistedCommitPushConfigParameters(
          remote.name,
          assisted.rawPushURL,
          assisted.pushURLSource
        )

  if (typeof window === 'undefined') {
    throw new Error('Push requires the Rust backend')
  }
  const needsStreaming =
    progressCallback !== undefined ||
    options?.onTerminalOutputAvailable !== undefined
  {
    const controller = new AbortController()
    let failedHook = false
    const listeners = new Set<(chunk: TerminalOutput) => void>()
    const terminalChunks: string[] = []
    const notifyTerminal = (text: string) => {
      if (!text) return
      pushTerminalChunk(terminalChunks, 256 * 1024, text)
      for (const listener of listeners) {
        try {
          withoutRepositoryGitAccess(() => listener(text))
        } catch {
          /* Observers cannot terminate a push. */
        }
      }
    }
    const decoders = { stdout: new TextDecoder(), stderr: new TextDecoder() }
    const parser =
      progressCallback === undefined
        ? undefined
        : new FetchStreamProgressParser(progress => {
            const description =
              progress.kind === 'progress'
                ? progress.details.text
                : progress.text
            withoutRepositoryGitAccess(() =>
              progressCallback({
                kind: 'push',
                title: `Pushing to ${remote.name}`,
                description,
                value: progress.percent,
                remote: remote.name,
                branch: localBranch,
              })
            )
          }, new PushProgressParser())
    progressCallback?.({
      kind: 'push',
      title: `Pushing to ${remote.name}`,
      value: 0,
      remote: remote.name,
      branch: localBranch,
    })
    const proxy = backendRemoteProxy(await envForProxy(remote.url))
    try {
      const failure = await withRepositoryGitOperation(
        repository.path,
        'mutation',
        async () => {
          const verifyAssistedSpawn =
            assisted === undefined
              ? undefined
              : await assisted.prepareForSpawn()
          return withHooksEnv(
            hookEnvironment => {
              const hooks = rendererHookEnvironment(hookEnvironment)
              const useNativeHookCallbacks = hooks === undefined
              return backend().pushRepositoryRemote(
                repository.path,
                {
                  remote: remote.name,
                  expected_url: assisted?.pushURL ?? remote.url,
                  local_branch: localBranch,
                  remote_branch: remoteBranch,
                  tags: tagsToPush ?? [],
                  force_with_lease: options?.forceWithLease ?? false,
                  no_verify: options?.noVerify ?? false,
                  report_progress: needsStreaming,
                  require_hook_callbacks:
                    useNativeHookCallbacks &&
                    (options?.onHookProgress !== undefined ||
                      options?.onHookFailure !== undefined ||
                      options?.onTerminalOutputAvailable !== undefined),
                  interactive_hook_decision:
                    useNativeHookCallbacks &&
                    options?.onHookFailure !== undefined,
                  ...(proxy === undefined ? {} : { proxy }),
                  ...(hooks === undefined ? {} : { hook_environment: hooks }),
                  ...(options?.trampoline === undefined
                    ? {}
                    : { trampoline: options.trampoline }),
                  ...(assisted === undefined ||
                  assistedConfigParameters === undefined
                    ? {}
                    : {
                        assisted: {
                          expected_tip: assisted.expectedTip,
                          remote_ref: assisted.remoteRef,
                          raw_push_url: assisted.rawPushURL,
                          push_url_source: assisted.pushURLSource,
                          configured_url: remote.url,
                          config_parameters: assistedConfigParameters,
                        },
                      }),
                },
                !needsStreaming
                  ? undefined
                  : chunk => {
                      try {
                        parser?.push(chunk)
                      } catch {
                        /* Progress observers cannot terminate a push. */
                      }
                      if (chunk.stream === 'lfs') return
                      const bytes = Uint8Array.from(
                        atob(chunk.data),
                        character => character.charCodeAt(0)
                      )
                      const text = decoders[chunk.stream].decode(bytes, {
                        stream: true,
                      })
                      notifyTerminal(text)
                    },
                options?.onTerminalOutputAvailable !== undefined
                  ? async () => {
                      try {
                        withoutRepositoryGitAccess(() =>
                          options?.onTerminalOutputAvailable?.(listener => {
                            for (const chunk of terminalChunks) {
                              try {
                                withoutRepositoryGitAccess(() =>
                                  listener(chunk)
                                )
                              } catch {
                                /* Observers cannot terminate a push. */
                              }
                            }
                            listeners.add(listener)
                            return {
                              unsubscribe: () => listeners.delete(listener),
                            }
                          })
                        )
                      } catch {
                        /* Observer setup must not terminate a push. */
                      }
                    }
                  : undefined,
                event => {
                  if (event.status === 'failed') failedHook = true
                  try {
                    withoutRepositoryGitAccess(() =>
                      options?.onHookProgress?.(
                        event.status === 'started'
                          ? {
                              hookName: event.hookName,
                              status: 'started',
                              abort: () => controller.abort(),
                            }
                          : { hookName: event.hookName, status: event.status }
                      )
                    )
                  } catch {
                    /* Hook observers cannot affect an active Git operation. */
                  }
                },
                options?.onHookProgress === undefined
                  ? undefined
                  : controller.signal,
                options?.onHookFailure === undefined
                  ? undefined
                  : output =>
                      withoutRepositoryGitAccess(() =>
                        options.onHookFailure!('pre-push', Buffer.from(output))
                      ),
                assisted === undefined
                  ? undefined
                  : async () => verifyAssistedSpawn?.()
              )
            },
            repository.path,
            {
              env:
                assistedConfigParameters === undefined
                  ? {}
                  : { GIT_CONFIG_PARAMETERS: assistedConfigParameters },
              interceptHooks: ['pre-push'],
              onHookProgress: options?.onHookProgress,
              onHookFailure: options?.onHookFailure,
            }
          )
        },
        true
      )
      if (failure !== undefined) {
        const notice =
          'Push failed or was rejected. Some destinations or tags may have been updated; inspect the remote before retrying.'
        let error: GitError
        if (isPushErrorKind(failure.errorKind)) {
          const kind = failure.errorKind
          const gitError = kind === 'push_protection' ? null : pushErrors[kind]
          const description =
            kind === 'push_protection'
              ? ' Push protection rejected the update; inspect the remote for details.'
              : ''
          error = new GitError(
            {
              exitCode: failure.exitCode,
              stdout: '',
              stderr: '',
              path: repository.path,
              gitError,
              gitErrorDescription: `${notice}${description}`,
            },
            ['push', '--', remote.name],
            ''
          )
        } else {
          error = fetchFailureToGitError(
            { exitCode: failure.exitCode, errorKind: failure.errorKind },
            repository.path,
            remote.name
          )
          error.message = `${notice} ${error.message}`
        }
        if (failedHook || failure.hookFailed) {
          error.message +=
            ' The pre-push hook failed. Ignoring an individual failed hook is not supported by the native backend.'
        }
        uncertainPushes.add(error)
        throw error
      }
      return
    } catch (error) {
      if (
        error instanceof BackendError &&
        error.code === 'push_outcome_uncertain'
      )
        uncertainPushes.add(error)
      throw error
    } finally {
      try {
        parser?.finish()
      } catch {
        /* Progress observers cannot invalidate a completed push. */
      }
      for (const decoder of Object.values(decoders)) {
        const text = decoder.decode()
        notifyTerminal(text)
      }
      listeners.clear()
    }
  }
}
