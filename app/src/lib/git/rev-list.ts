import type { Repository } from '../../models/repository'
import { type Branch, BranchType, type IAheadBehind } from '../../models/branch'
import type { CommitOneLine } from '../../models/commit'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Convert two refs into the Git range syntax representing the set of commits
 * that are reachable from `to` but excluding those that are reachable from
 * `from`. This will not be inclusive to the `from` ref, see
 * `revRangeInclusive`.
 *
 * Each parameter can be the commit SHA or a ref name, or specify an empty
 * string to represent HEAD.
 *
 * @param from The start of the range
 * @param to The end of the range
 */
export function revRange(from: string, to: string) {
  return `${from}..${to}`
}

/**
 * Convert two refs into the Git range syntax representing the set of commits
 * that are reachable from `to` but excluding those that are reachable from
 * `from`. However as opposed to `revRange`, this will also include `from` ref.
 *
 * Each parameter can be the commit SHA or a ref name, or specify an empty
 * string to represent HEAD.
 *
 * @param from The start of the range
 * @param to The end of the range
 */
export function revRangeInclusive(from: string, to: string) {
  return `${from}^..${to}`
}

/**
 * Convert two refs into the Git symmetric difference syntax, which represents
 * the set of commits that are reachable from either `from` or `to` but not
 * from both.
 *
 * Each parameter can be the commit SHA or a ref name, or you can use an empty
 * string to represent HEAD.
 *
 * @param from The start of the range
 * @param to The end of the range
 */
export function revSymmetricDifference(from: string, to: string) {
  return `${from}...${to}`
}

/** Calculate the number of commits the range is ahead and behind. */
export async function getAheadBehind(
  repository: Repository,
  range: string
): Promise<IAheadBehind | null> {
  const read = () => backend().repositoryAheadBehind(repository.path, range)
  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(
        repository.path,
        'read',
        read,
        true
      )
    : read()
}

/** Calculate the number of commits `branch` is ahead/behind its upstream. */
export async function getBranchAheadBehind(
  repository: Repository,
  branch: Branch
): Promise<IAheadBehind | null> {
  if (branch.type === BranchType.Remote) {
    return null
  }

  const upstream = branch.upstream
  if (!upstream) {
    return null
  }

  // NB: The three dot form means we'll go all the way back to the merge base
  // of the branch and its upstream. Practically this is important for seeing
  // "through" merges.
  const range = revSymmetricDifference(branch.name, upstream)
  return getAheadBehind(repository, range)
}

/**
 * Get a list of commits from the target branch that do not exist on the base
 * branch, ordered how they will be applied to the base branch.
 * Therefore, this will not include the baseBranchSha commit.
 *
 * This emulates how `git rebase` initially determines what will be applied to
 * the repository.
 *
 * Returns `null` when the rebase is not possible to perform, because of a
 * missing commit ID
 */
export async function getCommitsBetweenCommits(
  repository: Repository,
  baseBranchSha: string,
  targetBranchSha: string
): Promise<ReadonlyArray<CommitOneLine> | null> {
  const range = revRange(baseBranchSha, targetBranchSha)

  return getCommitsInRange(repository, range)
}

/**
 * Get a list of commits inside the provided range.
 *
 * Returns `null` when it is not possible to perform because of a bad range.
 */
export async function getCommitsInRange(
  repository: Repository,
  range: string
): Promise<ReadonlyArray<CommitOneLine> | null> {
  const read = () => backend().repositoryRevisionList(repository.path, range)
  const stdout: string | null = backend().transportKind === 'wry'
    ? await withRepositoryGitOperation(
        repository.path,
        'read',
        read,
        true
      )
    : await read()
  if (stdout === null) return null

  const lines = stdout.split('\n')

  const commits: CommitOneLine[] = []

  const commitSummaryRe = /^([a-f0-9]{40}|[a-f0-9]{64}) (.*)$/

  for (const line of lines) {
    const match = commitSummaryRe.exec(line)

    if (match !== null && match.length === 3) {
      const sha = match[1]
      const summary = match[2]

      commits.push({
        sha,
        summary,
      })
    }
  }

  return commits
}

/**
 * Determine if merge commits exist in history after given commit
 * If commitRef is null, goes back to HEAD of branch.
 */
export async function doMergeCommitsExistAfterCommit(
  repository: Repository,
  commitRef: string | null
): Promise<boolean> {
  const revision = commitRef === null ? 'HEAD' : revRange(commitRef, 'HEAD')
  const read = () => backend().repositoryHasMergeCommits(repository.path, revision)
  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(
        repository.path,
        'read',
        read,
        true
      )
    : read()
}
