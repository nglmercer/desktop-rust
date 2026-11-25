import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Get the `limit` most recently checked out branches.
 */
export async function getRecentBranches(
  repository: Repository,
  limit: number
): Promise<ReadonlyArray<string>> {
  // "git reflog show" is just an alias for "git log -g --abbrev-commit --pretty=oneline"
  // but by using log we can give it a max number which should prevent us from balling out
  // of control when there's ginormous reflogs around (as in e.g. github/github).
  const regex = new RegExp(
    /.*? (renamed|checkout)(?:: moving from|\s*) (?:refs\/heads\/|\s*)(.*?) to (?:refs\/heads\/|\s*)(.*?)$/i
  )

  const read = () =>
    backend().repositoryRendererReflog(repository.path, 'branches')
  const result =
    backend().transportKind === 'wry'
      ? await withRepositoryGitOperation(repository.path, 'read', read, true)
      : await read()
  if (result === null) return []
  const stdout = result

  const lines = stdout.split('\n')
  const names = new Set<string>()
  const excludedNames = new Set<string>()

  for (const line of lines) {
    const result = regex.exec(line)
    if (result && result.length === 4) {
      const operationType = result[1]
      const excludeBranchName = result[2]
      const branchName = result[3]

      if (operationType === 'renamed') {
        // exclude intermediate-state renaming branch from recent branches
        excludedNames.add(excludeBranchName)
      }

      if (!excludedNames.has(branchName)) {
        names.add(branchName)
      }
    }

    if (names.size === limit) {
      break
    }
  }

  return [...names]
}

/**
 * Gets the distinct list of branches that have been checked out after a specific date
 * Returns a map keyed on branch names
 *
 * @param repository the repository who's reflog you want to check
 * @param afterDate filters checkouts so that only those occurring on or after this date are returned
 * @returns map of branch name -> checkout date
 */
export async function getBranchCheckouts(
  repository: Repository,
  afterDate: Date
): Promise<Map<string, Date>> {
  //regexr.com/46n1v
  const regex = new RegExp(
    /^(?:[a-f0-9]{40}|[a-f0-9]{64})\sHEAD@{(.*)}\scheckout: moving from\s.*\sto\s(.*)$/
  )
  const read = () =>
    backend().repositoryBranchCheckouts(repository.path, afterDate)
  const result =
    backend().transportKind === 'wry'
      ? await withRepositoryGitOperation(repository.path, 'read', read, true)
      : await read()
  if (result === null) return new Map()
  const stdout = result

  const checkouts = new Map<string, Date>()
  const lines = stdout.split('\n')
  for (const line of lines) {
    const parsedLine = regex.exec(line)

    if (parsedLine === null || parsedLine.length !== 3) {
      continue
    }

    const [, timestamp, branchName] = parsedLine
    if (!checkouts.has(branchName)) {
      checkouts.set(branchName, new Date(timestamp))
    }
  }

  return checkouts
}
