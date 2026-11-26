import type { IChildProcess } from '../process/child-process'
import { GitErrorKind } from './error-kind'
import { forEachLine } from '../line-stream'

import { Repository } from '../../models/repository'
import { RebaseInternalState, RebaseProgressOptions } from '../../models/rebase'
import { IMultiCommitOperationProgress } from '../../models/progress'
import {
  WorkingDirectoryFileChange,
  AppFileStatusKind,
} from '../../models/status'
import { ManualConflictResolution } from '../../models/manual-conflict-resolution'
import { Commit, CommitOneLine } from '../../models/commit'

import { merge } from '../merge'
import { formatRebaseValue } from '../rebase'

import {
  git,
  IGitExecutionOptions,
  IGitStringExecutionOptions,
  IGitStringResult,
  HookCallbackOptions,
} from './core'
import { stageManualConflictResolution } from './stage'
import { stageFiles } from './update-index'
import { getStatus } from './status'
import { getCommitsBetweenCommits } from './rev-list'
import { Branch } from '../../models/branch'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/** The app-specific results from attempting to rebase a repository */
export enum RebaseResult {
  /**
   * Git completed the rebase without reporting any errors, and the caller can
   * signal success to the user.
   */
  CompletedWithoutError = 'CompletedWithoutError',
  /**
   * Git completed the rebase without reporting any errors, but the branch was
   * already up to date and there was nothing to do.
   */
  AlreadyUpToDate = 'AlreadyUpToDate',
  /**
   * The rebase encountered conflicts while attempting to rebase, and these
   * need to be resolved by the user before the rebase can continue.
   */
  ConflictsEncountered = 'ConflictsEncountered',
  /**
   * The rebase was not able to continue as tracked files were not staged in
   * the index.
   */
  OutstandingFilesNotStaged = 'OutstandingFilesNotStaged',
  /**
   * The rebase was not attempted because it could not check the status of the
   * repository. The caller needs to confirm the repository is in a usable
   * state.
   */
  Aborted = 'Aborted',
  /**
   * An unexpected error as part of the rebase flow was caught and handled.
   *
   * Check the logs to find the relevant Git details.
   */
  Error = 'Error',
}

/**
 * Get the internal state about the rebase being performed on a repository. This
 * information is required to help Desktop display information to the user
 * about the current action as well as the options available.
 *
 * Returns `null` if no rebase is detected, or if the expected information
 * cannot be found in the repository.
 */
export async function getRebaseInternalState(
  repository: Repository
): Promise<RebaseInternalState | null> {
  const state = await withRepositoryGitOperation(
    repository.path,
    'read',
    () => backend().repositoryRebaseState(repository.path),
    true
  )
  const progress = state?.progress
  if (
    progress?.original == null ||
    progress.branch == null ||
    progress.onto == null
  ) {
    return null
  }
  return {
    originalBranchTip: progress.original.trim(),
    targetBranch: progress.branch.startsWith('refs/heads/')
      ? progress.branch.substring(11).trim()
      : progress.branch,
    baseBranchTip: progress.onto.trim(),
  }
}

/**
 * Inspect the `.git/rebase-merge` folder and convert the current rebase state
 * into data that can be provided to the rebase flow to update the application
 * state.
 *
 * This is required when Desktop is not responsible for initiating the rebase:
 *
 *   - when a rebase outside Desktop encounters conflicts
 *   - when a `git pull --rebase` was run and encounters conflicts
 *
 */
export async function getRebaseSnapshot(repository: Repository): Promise<{
  progress: IMultiCommitOperationProgress
  commits: ReadonlyArray<CommitOneLine>
} | null> {
  const state = await withRepositoryGitOperation(
    repository.path,
    'read',
    () => backend().repositoryRebaseState(repository.path),
    true
  )
  const progressState = state?.progress
  if (
    progressState?.current == null ||
    progressState.total == null ||
    progressState.original == null ||
    progressState.onto == null
  ) {
    return null
  }

  const next = parseInt(progressState.current, 10)
  const last = parseInt(progressState.total, 10)
  if (isNaN(next) || isNaN(last)) {
    log.warn(
      `[getCurrentProgress] found invalid rebase progress '${progressState.current}/${progressState.total}'`
    )
    return null
  }
  const originalBranchTip = progressState.original.trim()
  const baseBranchTip = progressState.onto.trim()
  if (
    next <= 0 ||
    last <= 0 ||
    originalBranchTip === '' ||
    baseBranchTip === ''
  ) {
    return null
  }

  const commits = await getCommitsBetweenCommits(
    repository,
    baseBranchTip,
    originalBranchTip
  )
  if (commits === null || commits.length === 0) {
    return null
  }

  const nextCommitIndex = next - 1
  const currentCommitSummary =
    nextCommitIndex >= 0 && nextCommitIndex < commits.length
      ? commits[nextCommitIndex].summary
      : ''
  return {
    progress: {
      kind: 'multiCommitOperation',
      value: formatRebaseValue(next / last),
      position: next,
      totalCommitCount: last,
      currentCommitSummary,
    },
    commits,
  }
}

/**
 * Attempt to read the `.git/REBASE_HEAD` file inside a repository to confirm
 * the rebase is still active.
 */
async function readRebaseHead(repository: Repository): Promise<string | null> {
  try {
    const state = await withRepositoryGitOperation(
      repository.path,
      'read',
      () => backend().repositoryRebaseState(repository.path),
      true
    )
    return state?.head ?? null
  } catch (error) {
    log.warn(
      '[rebase] a problem was encountered reading rebase state, so it is unsafe to continue rebasing',
      error
    )
    return null
  }
}

/** Regex for identifying when rebase applied each commit onto the base branch */
const rebasingRe = /^Rebasing \((\d+)\/(\d+)\)$/

/**
 * A parser to read and emit rebase progress from Git `stderr`
 */
class GitRebaseParser {
  public constructor(private readonly commits: ReadonlyArray<CommitOneLine>) {}

  public parse(line: string): IMultiCommitOperationProgress | null {
    const match = rebasingRe.exec(line)
    if (match === null || match.length !== 3) {
      // Git will sometimes emit other output (for example, when it tries to
      // resolve conflicts) and this does not match the expected output
      return null
    }

    const rebasedCommitCount = parseInt(match[1], 10)
    const totalCommitCount = parseInt(match[2], 10)

    if (isNaN(rebasedCommitCount) || isNaN(totalCommitCount)) {
      return null
    }

    const currentCommitSummary =
      this.commits[rebasedCommitCount - 1]?.summary ?? ''

    const progress = rebasedCommitCount / totalCommitCount
    const value = formatRebaseValue(progress)

    return {
      kind: 'multiCommitOperation',
      value,
      position: rebasedCommitCount,
      totalCommitCount: totalCommitCount,
      currentCommitSummary,
    }
  }
}

function configureOptionsForRebase<T extends IGitExecutionOptions>(
  options: T,
  progress?: RebaseProgressOptions
) {
  if (progress === undefined) {
    return options
  }

  const { commits, progressCallback } = progress

  return merge(options, {
    processCallback: (process: IChildProcess) => {
      // If Node.js encounters a synchronous runtime error while spawning
      // `stderr` will be undefined and the error will be emitted asynchronously
      if (process.stderr === null) {
        return
      }
      const parser = new GitRebaseParser(commits)

      forEachLine(process.stderr, line => {
        const progress = parser.parse(line)

        if (progress != null) {
          progressCallback(progress)
        }
      })
    },
  })
}

/**
 * A stub function to use for initiating rebase in the app.
 *
 * If the rebase fails, the repository will be in an indeterminate state where
 * the rebase is stuck.
 *
 * If the rebase completes without error, `featureBranch` will be checked out
 * and it will probably have a different commit history.
 *
 * @param baseBranch the ref to start the rebase from
 * @param targetBranch the ref to rebase onto `baseBranch`
 */
export async function rebase(
  repository: Repository,
  baseBranch: Branch,
  targetBranch: Branch,
  progressCallback?: (progress: IMultiCommitOperationProgress) => void,
  expected?: { readonly head: string; readonly branch: string | null }
): Promise<RebaseResult> {
  if (expected === undefined) {
    throw new Error('Refresh the current branch before rebasing')
  }
  const request = {
    base_ref: baseBranch.ref,
    base_head: baseBranch.tip.sha,
    target_ref: targetBranch.ref,
    target_head: targetBranch.tip.sha,
    current_head: expected.head,
    current_branch: expected.branch,
  }
  // The native rebase does not stream its steps yet. Read the commits it will
  // replay now and report each step once the operation has returned.
  const replayed =
    progressCallback === undefined
      ? null
      : await getCommitsBetweenCommits(
          repository,
          baseBranch.tip.sha,
          targetBranch.tip.sha
        )
  const result = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().startRepositoryRebase(repository.path, request),
    true
  )
  if (
    progressCallback !== undefined &&
    replayed !== null &&
    result !== 'up_to_date'
  ) {
    const steps =
      result === 'conflict'
        ? ((await getRebaseSnapshot(repository))?.progress.position ?? 0)
        : replayed.length
    const parser = new GitRebaseParser(replayed)
    for (let step = 1; step <= steps; step++) {
      const progress = parser.parse(`Rebasing (${step}/${replayed.length})`)
      if (progress !== null) progressCallback(progress)
    }
  }
  return result === 'conflict'
    ? RebaseResult.ConflictsEncountered
    : result === 'up_to_date'
      ? RebaseResult.AlreadyUpToDate
      : RebaseResult.CompletedWithoutError
}

/** Abandon the current rebase operation */
export async function abortRebase(repository: Repository) {
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().abortRepositoryGitOperation(repository.path, 'rebase'),
    true
  )
}

function parseRebaseResult(result: IGitStringResult): RebaseResult {
  if (result.exitCode === 0) {
    if (result.stdout.trim().match(/^Current branch [^ ]+ is up to date.$/i)) {
      return RebaseResult.AlreadyUpToDate
    }

    return RebaseResult.CompletedWithoutError
  }

  if (result.gitError === GitErrorKind.RebaseConflicts) {
    return RebaseResult.ConflictsEncountered
  }

  if (result.gitError === GitErrorKind.UnresolvedConflicts) {
    return RebaseResult.OutstandingFilesNotStaged
  }

  throw new Error(`Unhandled result found: '${JSON.stringify(result)}'`)
}

/**
 * Proceed with the current rebase operation and report back on whether it completed
 *
 * It is expected that the index has staged files which are cleanly rebased onto
 * the base branch, and the remaining unstaged files are those which need manual
 * resolution or were changed by the user to address inline conflicts.
 *
 */
export async function continueRebase(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  manualResolutions: ReadonlyMap<string, ManualConflictResolution> = new Map(),
  opts?: RebaseInteractiveOptions
): Promise<RebaseResult> {
  if (opts?.gitEditor !== undefined && opts.gitEditor !== ':') {
    throw new Error(
      'Custom rebase editors are not supported by the native backend'
    )
  }
  const initialState = await withRepositoryGitOperation(
    repository.path,
    'read',
    () => backend().repositoryRebaseState(repository.path),
    true
  )
  if (initialState === null) {
    return RebaseResult.Aborted
  }
  if (initialState.backend !== 'merge') {
    throw new Error(
      'Continuing an apply-backend rebase is not supported; use Git to continue or abort it'
    )
  }

  const trackedFiles = files.filter(
    file => file.status.kind !== AppFileStatusKind.Untracked
  )
  for (const [path, resolution] of manualResolutions) {
    const file = files.find(file => file.path === path)
    if (file !== undefined) {
      await stageManualConflictResolution(repository, file, resolution)
    } else {
      log.error(
        `[continueRebase] couldn't find file ${path} even though there's a manual resolution for it`
      )
    }
  }
  await stageFiles(
    repository,
    trackedFiles.filter(file => !manualResolutions.has(file.path))
  )

  const status = await getStatus(repository, false)
  if (status === null) {
    log.warn(
      '[continueRebase] unable to get status after staging changes, skipping any other steps'
    )
    return RebaseResult.Aborted
  }
  const rebaseCurrentCommit = await readRebaseHead(repository)
  if (rebaseCurrentCommit === null) {
    return RebaseResult.Aborted
  }
  if (initialState.head !== rebaseCurrentCommit) {
    throw new Error('Rebase changed; refresh before continuing')
  }

  const trackedFilesAfter = status.workingDirectory.files.filter(
    file => file.status.kind !== AppFileStatusKind.Untracked
  )
  const before = opts?.progressCallback
    ? await getRebaseSnapshot(repository)
    : null
  let result: 'complete' | 'conflict'
  try {
    result = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().continueRepositoryRebase(
          repository.path,
          initialState.head,
          trackedFilesAfter.length === 0,
          opts?.noVerify === true
        ),
      true
    )
  } catch (error) {
    if (error instanceof Error) {
      if (error.message === 'Resolve and stage conflicts before continuing') {
        return RebaseResult.OutstandingFilesNotStaged
      }
      if (
        error.message.startsWith('Unable to continue rebase') &&
        trackedFilesAfter.some(
          changed => !files.some(listed => listed.path === changed.path)
        )
      ) {
        return RebaseResult.OutstandingFilesNotStaged
      }
    }
    throw error
  }

  // The native rebase does not stream its steps yet; report the ones it
  // applied now that it has returned.
  if (opts?.progressCallback && before !== null) {
    const total = before.progress.totalCommitCount
    const reached =
      result === 'complete'
        ? total
        : ((await getRebaseSnapshot(repository))?.progress.position ??
          before.progress.position)
    const parser = new GitRebaseParser(before.commits)
    for (let step = before.progress.position + 1; step <= reached; step++) {
      const progress = parser.parse(`Rebasing (${step}/${total})`)
      if (progress !== null) opts.progressCallback(progress)
    }
  }
  return result === 'complete'
    ? RebaseResult.CompletedWithoutError
    : RebaseResult.ConflictsEncountered
}

export type RebaseInteractiveOptions = {
  /**
   * a description of the action to be displayed in the progress dialog - i.e. Squash, Amend, etc..
   */
  action?: string

  /**
   * the GIT_EDITOR environment variable to use during the interactive rebase,
   * defaults to ':' which is a no-op command
   */
  gitEditor?: string
  progressCallback?: (progress: IMultiCommitOperationProgress) => void
  commits?: ReadonlyArray<Commit>
  noVerify?: boolean
} & HookCallbackOptions

/**
 * Method for initiating interactive rebase in the app.
 *
 * In order to modify the interactive todo list during interactive rebase, we
 * create a temporary todo list of our own. Pass that file's path into our
 * interactive rebase and using the sequence.editor to cat replace the
 * interactive todo list with the contents of our generated one.
 *
 * @param pathOfGeneratedTodo path to generated todo list for interactive rebase
 * @param lastRetainedCommitRef the commit before the earliest commit to be
 * changed during the interactive rebase or null if commit is root (first commit
 * in history) of branch
 */
export async function rebaseInteractive(
  repository: Repository,
  pathOfGeneratedTodo: string,
  lastRetainedCommitRef: string | null,
  opts?: RebaseInteractiveOptions
): Promise<RebaseResult> {
  const baseOptions: IGitStringExecutionOptions = {
    expectedErrors: new Set([GitErrorKind.RebaseConflicts]),
    env: {
      GIT_SEQUENCE_EDITOR: undefined,
      GIT_EDITOR: opts?.gitEditor ?? ':',
    },
  }

  let options = baseOptions

  const { progressCallback, commits } = opts ?? {}

  if (progressCallback) {
    if (!commits) {
      log.warn(`Unable to interactively rebase if no commits`)
      return RebaseResult.Error
    }

    options = configureOptionsForRebase(baseOptions, {
      commits,
      progressCallback,
    })
  }

  options = {
    ...options,
    onHookProgress: opts?.onHookProgress,
    onHookFailure: opts?.onHookFailure,
    onTerminalOutputAvailable: opts?.onTerminalOutputAvailable,
  }

  /* If the commit is the first commit in the branch, we cannot reference it
  using the sha thus if lastRetainedCommitRef is null (we couldn't define it),
  we must use the --root flag */
  const ref = lastRetainedCommitRef == null ? '--root' : lastRetainedCommitRef
  const result = await git(
    [
      '-c',
      // This replaces interactive todo with contents of file at pathOfGeneratedTodo
      `sequence.editor=cat "${pathOfGeneratedTodo}" >`,
      'rebase',
      ...(opts?.noVerify ? ['--no-verify'] : []),
      '-i',
      ref,
    ],
    repository.path,
    opts?.action ?? 'Interactive rebase',
    options
  )

  return parseRebaseResult(result)
}
