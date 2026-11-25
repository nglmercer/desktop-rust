import { HookCallbackOptions } from './core'
import { stageFiles } from './update-index'
import { Repository } from '../../models/repository'
import { WorkingDirectoryFileChange } from '../../models/status'
import { ManualConflictResolution } from '../../models/manual-conflict-resolution'
import { stageManualConflictResolution } from './stage'
import { ITextDiff, ILargeTextDiff } from '../../models/diff'
import { backend } from '../../backend/client'
import {
  withRepositoryGitOperation,
  withoutRepositoryGitAccess,
} from './repository-operation'
import type { ISelectedCommitReviewWithSelection } from './selected-commit-review'

/**
 * @param repository repository to execute merge in
 * @param message commit message
 * @param files files to commit
 * @returns the commit SHA
 */
export async function createCommit(
  repository: Repository,
  message: string,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  options?: {
    amend?: boolean
    noVerify?: boolean
    signOff?: boolean
    allowEmpty?: boolean
    /** Preserve the verified diff basis of restored partial selections through staging. */
    readonly certifiedPartialDiffs?: ReadonlyMap<
      string,
      ITextDiff | ILargeTextDiff
    >
    readonly selectedReview?: ISelectedCommitReviewWithSelection
    readonly allowLargeFiles?: boolean
  } & HookCallbackOptions
): Promise<string> {
  const review = options?.selectedReview
  if (review === undefined) {
    throw new Error('The selected changes must be reviewed before committing.')
  }
  const controller = new AbortController()
  const listeners = new Set<(chunk: string | Buffer | Buffer[]) => void>()
  const buffered: Buffer[] = []
  let bufferedBytes = 0
  let failedHook: string | undefined
  const notify = (operation: () => void) => {
    try {
      withoutRepositoryGitAccess(operation)
    } catch (error) {
      log.error('Commit observer failed', error)
    }
  }
  notify(() =>
    options?.onTerminalOutputAvailable?.(callback => {
      for (const chunk of buffered) notify(() => callback(chunk))
      listeners.add(callback)
      return { unsubscribe: () => listeners.delete(callback) }
    })
  )
  const checkpoint = await backend().prepareSelectedCommit(
    repository.path,
    review.selection,
    review.basis,
    { signal: controller.signal }
  )
  try {
    const result = await backend().publishSelectedCommit(
      repository.path,
      checkpoint,
      message,
      {
        noVerify: options?.noVerify,
        signOff: options?.signOff,
        allowLargeFiles: options?.allowLargeFiles,
        signal: controller.signal,
        onHookProgress: progress => {
          if (progress.status === 'failed') failedHook = progress.hookName
          notify(() =>
            options?.onHookProgress?.(
              progress.status === 'started'
                ? {
                    hookName: progress.hookName,
                    status: 'started',
                    abort: () => controller.abort(),
                  }
                : {
                    hookName: progress.hookName,
                    status: progress.status as 'finished' | 'failed',
                  }
            )
          )
        },
        onOutput: output => {
          const chunk = Buffer.from(output.bytes)
          if (bufferedBytes + chunk.length <= 256 * 1024) {
            buffered.push(chunk)
            bufferedBytes += chunk.length
          }
          for (const listener of listeners) notify(() => listener(chunk))
        },
      }
    )
    return result.oid
  } catch (error) {
    if (failedHook !== undefined) {
      throw new Error(
        `The ${failedHook} hook failed. Inspect commit checkpoint ${checkpoint} before retrying. Ignoring an individual failed hook is not supported by the native backend.`,
        { cause: error }
      )
    }
    throw new Error(
      `Commit outcome may be uncertain. Inspect commit checkpoint ${checkpoint} before retrying.`,
      { cause: error }
    )
  }
}

/**
 * Creates a commit to finish an in-progress merge
 * assumes that all conflicts have already been resolved
 * *Warning:* Does _not_ clear staged files before it commits!
 *
 * @param repository repository to execute merge in
 * @param files files to commit
 */
export async function createMergeCommit(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  manualResolutions: ReadonlyMap<string, ManualConflictResolution> = new Map()
): Promise<string> {
  // apply manual conflict resolutions
  for (const [path, resolution] of manualResolutions) {
    const file = files.find(f => f.path === path)
    if (file !== undefined) {
      await stageManualConflictResolution(repository, file, resolution)
    } else {
      log.error(
        `couldn't find file ${path} even though there's a manual resolution for it`
      )
    }
  }

  const otherFiles = files.filter(f => !manualResolutions.has(f.path))

  await stageFiles(repository, otherFiles)
  const stdout = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().finishRepositoryMergeCommit(repository.path),
    true
  )
  return stdout.split(']')[0].split(' ')[1]
}
