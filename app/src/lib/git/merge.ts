import { join } from 'path'
import type { HookCallbackOptions } from './core'
import { Repository } from '../../models/repository'
import { pathExists } from '../path-exists'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

export enum MergeResult {
  /** The merge completed successfully */
  Success,
  /**
   * The merge was a noop since the current branch
   * was already up to date with the target branch.
   */
  AlreadyUpToDate,
  /**
   * The merge failed, likely due to conflicts.
   */
  Failed,
}

export type MergeOptions = {
  /** Whether to perform a squash merge */
  readonly squash?: boolean
  /** Whether to bypass pre-merge and post-merge hooks */
  readonly noVerify?: boolean
} & HookCallbackOptions

/** Merge the named branch into the current branch. */
export async function merge(
  repository: Repository,
  branch: string,
  _options?: MergeOptions,
  expected?: {
    readonly sourceRef: string
    readonly sourceHead: string
    readonly currentRef: string
    readonly currentHead: string
  }
): Promise<MergeResult> {
  if (expected === undefined) {
    throw new Error('Refresh the current and source branches before merging')
  }
  const request = {
    source_ref: expected.sourceRef,
    source_head: expected.sourceHead,
    current_ref: expected.currentRef,
    current_head: expected.currentHead,
    squash: _options?.squash === true,
    no_verify: _options?.noVerify === true,
  }
  const result = await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().startRepositoryMerge(repository.path, request),
    true
  )
  return result === 'conflict'
    ? MergeResult.Failed
    : result === 'up_to_date'
      ? MergeResult.AlreadyUpToDate
      : MergeResult.Success
}

/**
 * Find the base commit between two commit-ish identifiers
 *
 * @returns the commit id of the merge base, or null if the two commit-ish
 *          identifiers do not have a common base
 */
export async function getMergeBase(
  repository: Repository,
  firstCommitish: string,
  secondCommitish: string
): Promise<string | null> {
  const read = () =>
    backend().repositoryMergeBase(
      repository.path,
      firstCommitish,
      secondCommitish
    )
  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(repository.path, 'read', read, true)
    : read()
}

/** Abort a mid-flight (conflicted) merge. */
export async function abortMerge(repository: Repository): Promise<void> {
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().abortRepositoryGitOperation(repository.path, 'merge'),
    true
  )
}

/** Check the `.git/MERGE_HEAD` file exists in a repository. */
export async function isMergeHeadSet(repository: Repository): Promise<boolean> {
  const path = join(repository.resolvedGitDir, 'MERGE_HEAD')
  return await pathExists(path)
}

/** Check whether `.git/SQUASH_MSG` exists (a squash merge with conflicts). */
export async function isSquashMsgSet(repository: Repository): Promise<boolean> {
  const path = join(repository.resolvedGitDir, 'SQUASH_MSG')
  return await pathExists(path)
}
