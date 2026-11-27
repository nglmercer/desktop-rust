import { Repository } from '../../models/repository'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../models/status'
import { getStatus } from './status'
import { stageFiles } from './update-index'
import { getCommitsInRange, revRange } from './rev-list'
import { CommitOneLine } from '../../models/commit'
import { round } from '../../ui/lib/round'
import { ICherryPickSnapshot } from '../../models/cherry-pick'
import { ManualConflictResolution } from '../../models/manual-conflict-resolution'
import { stageManualConflictResolution } from './stage'
import { getCommit } from '.'
import { IMultiCommitOperationProgress } from '../../models/progress'
import { backend } from '../../backend/client'
import {
  withoutRepositoryGitAccess,
  withRepositoryGitOperation,
} from './repository-operation'
import { OutputLineStream } from '../progress/fetch-stream'

/** The app-specific results from attempting to cherry pick commits*/
export enum CherryPickResult {
  /** Git completed the cherry pick without reporting any errors. */
  CompletedWithoutError = 'CompletedWithoutError',
  /** The cherry pick encountered conflicts that must be resolved. */
  ConflictsEncountered = 'ConflictsEncountered',
  /** Tracked files were not staged in the index. */
  OutstandingFilesNotStaged = 'OutstandingFilesNotStaged',
  /** The cherry pick was not or could not be started. */
  UnableToStart = 'UnableToStart',
  /** An unexpected error was caught and handled. */
  Error = 'Error',
}

/** Parse successful cherry-pick steps for user-facing progress. */
class GitCherryPickParser {
  public constructor(
    private readonly commits: ReadonlyArray<CommitOneLine>,
    private count: number = 0
  ) {}

  public parse(line: string): IMultiCommitOperationProgress | null {
    if (!/^\[(.*\s.*)\]/.test(line)) {
      return null
    }
    this.count++
    return {
      kind: 'multiCommitOperation',
      value: round(this.count / this.commits.length, 2),
      position: this.count,
      totalCommitCount: this.commits.length,
      currentCommitSummary: this.commits[this.count - 1]?.summary ?? '',
    }
  }
}

function cherryPickProgressStream(
  commits: ReadonlyArray<CommitOneLine>,
  initialCount: number,
  callback: (progress: IMultiCommitOperationProgress) => void
): OutputLineStream {
  const parser = new GitCherryPickParser(commits, initialCount)
  return new OutputLineStream((_stream, line) => {
    const progress = parser.parse(line)
    if (progress === null) return
    try {
      withoutRepositoryGitAccess(() => callback(progress))
    } catch {
      // Progress observers cannot terminate a cherry-pick.
    }
  })
}

async function readCherryPickState(repository: Repository) {
  return withRepositoryGitOperation(
    repository.path,
    'read',
    () => backend().repositoryCherryPickState(repository.path),
    true
  )
}

/** Initiate cherry picking on the reviewed branch state. */
export async function cherryPick(
  repository: Repository,
  commits: ReadonlyArray<CommitOneLine>,
  progressCallback?: (progress: IMultiCommitOperationProgress) => void,
  expected?: { readonly head: string; readonly branch: string | null }
): Promise<CherryPickResult> {
  if (commits.length === 0) {
    return CherryPickResult.UnableToStart
  }
  if (expected === undefined) {
    throw new Error('Refresh the current branch before cherry-picking')
  }
  const progressStream =
    progressCallback === undefined
      ? undefined
      : cherryPickProgressStream(commits, 0, progressCallback)
  let result: 'complete' | 'conflict'
  try {
    result = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().startRepositoryCherryPick(
          repository.path,
          commits.map(commit => commit.sha),
          expected,
          progressStream === undefined
            ? undefined
            : chunk => progressStream.push(chunk)
        ),
      true
    )
  } finally {
    progressStream?.finish()
  }
  return result === 'complete'
    ? CherryPickResult.CompletedWithoutError
    : CherryPickResult.ConflictsEncountered
}

/**
 * Inspect the native cherry-pick state and convert it into progress and the
 * remaining commits queued for cherry picking.
 */
export async function getCherryPickSnapshot(
  repository: Repository
): Promise<ICherryPickSnapshot | null> {
  const state = await readCherryPickState(repository)
  if (state === null) {
    return null
  }

  if (state.sequence === null) {
    // A single cherry-pick may have no sequencer. Re-read the fixed state so a
    // concurrent continue/abort cannot make us describe the wrong commit.
    const current = await readCherryPickState(repository)
    if (current === null || current.head !== state.head) {
      return null
    }
    const commit = await getCommit(repository, state.head)
    if (commit === null) {
      return null
    }
    return {
      progress: {
        kind: 'multiCommitOperation',
        value: 1,
        position: 1,
        totalCommitCount: 1,
        currentCommitSummary: commit.summary,
      },
      remainingCommits: [],
      commits: [{ sha: commit.sha, summary: commit.summary }],
      targetBranchUndoSha: '',
      cherryPickedCount: 0,
    }
  }

  const abortSafetySha = state.sequence.abortSafety.trim()
  const headSha = state.sequence.head.trim()
  const remainingPicks = state.sequence.todo.trim()
  if (abortSafetySha === '' || headSha === '' || remainingPicks === '') {
    return null
  }

  const remainingCommits: CommitOneLine[] = []
  for (const pick of remainingPicks.split('\n')) {
    const line = pick.replace(/^pick /, '')
    if (!line.trim().includes(' ')) {
      continue
    }
    const sha = line.substring(0, line.indexOf(' '))
    remainingCommits.push({ sha, summary: line.substring(sha.length + 1) })
  }
  if (remainingCommits.length === 0) {
    return null
  }

  const commitsCherryPicked =
    abortSafetySha !== headSha
      ? await getCommitsInRange(repository, revRange(headSha, abortSafetySha))
      : []
  if (commitsCherryPicked === null) {
    return null
  }

  const commits = [...commitsCherryPicked, ...remainingCommits]
  const position = commitsCherryPicked.length + 1
  return {
    progress: {
      kind: 'multiCommitOperation',
      value: round(position / commits.length, 2),
      position,
      totalCommitCount: commits.length,
      currentCommitSummary: remainingCommits[0].summary ?? '',
    },
    remainingCommits,
    commits,
    targetBranchUndoSha: headSha,
    cherryPickedCount: commitsCherryPicked.length,
  }
}

/** Continue the active cherry-pick after staging conflict resolutions. */
export async function continueCherryPick(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  manualResolutions: ReadonlyMap<string, ManualConflictResolution> = new Map(),
  progressCallback?: (progress: IMultiCommitOperationProgress) => void
): Promise<CherryPickResult> {
  const expectedState = await readCherryPickState(repository)
  if (expectedState === null) {
    return CherryPickResult.UnableToStart
  }

  const trackedFiles = files.filter(
    file => file.status.kind !== AppFileStatusKind.Untracked
  )
  for (const [path, resolution] of manualResolutions) {
    const file = files.find(file => file.path === path)
    if (file === undefined) {
      log.error(
        `[continueCherryPick] couldn't find file ${path} even though there's a manual resolution for it`
      )
      continue
    }
    await stageManualConflictResolution(repository, file, resolution)
  }
  await stageFiles(
    repository,
    trackedFiles.filter(file => !manualResolutions.has(file.path))
  )

  const status = await getStatus(repository, false)
  if (status === null || !(await isCherryPickHeadFound(repository))) {
    return CherryPickResult.UnableToStart
  }
  const trackedFilesAfter = status.workingDirectory.files.filter(
    file => file.status.kind !== AppFileStatusKind.Untracked
  )
  const before =
    progressCallback === undefined
      ? null
      : await getCherryPickSnapshot(repository)

  const progressStream =
    progressCallback === undefined || before === null
      ? undefined
      : cherryPickProgressStream(
          before.commits,
          before.cherryPickedCount,
          progressCallback
        )
  let outcome: 'complete' | 'conflict'
  try {
    outcome = await withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().continueRepositoryCherryPick(
          repository.path,
          expectedState.head,
          trackedFilesAfter.length === 0,
          progressStream === undefined
            ? undefined
            : chunk => progressStream.push(chunk)
        ),
      true
    )
  } catch (error) {
    if (
      error instanceof Error &&
      error.message === 'Resolve and stage conflicts before continuing'
    ) {
      return CherryPickResult.OutstandingFilesNotStaged
    }
    throw error
  } finally {
    progressStream?.finish()
  }

  return outcome === 'complete'
    ? CherryPickResult.CompletedWithoutError
    : CherryPickResult.ConflictsEncountered
}

/** Abandon the current cherry-pick operation. */
export async function abortCherryPick(repository: Repository) {
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().abortRepositoryGitOperation(repository.path, 'cherry_pick'),
    true
  )
}

/** Check whether a cherry-pick is currently active. */
export async function isCherryPickHeadFound(
  repository: Repository
): Promise<boolean> {
  try {
    const context = await withRepositoryGitOperation(
      repository.path,
      'read',
      () => backend().repositoryStatusContext(repository.path),
      true
    )
    return context.isCherryPickingHeadFound
  } catch (error) {
    log.warn(
      '[cherryPick] a problem was encountered reading cherry-pick state, so it is unsafe to continue cherry-picking',
      error
    )
    return false
  }
}
