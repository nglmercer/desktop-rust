import { git } from './core'
import { Repository } from '../../models/repository'
import { Branch } from '../../models/branch'
import { formatAsLocalRef } from './refs'
import { deleteRef } from './update-ref'
import { GitError as DugiteError } from 'dugite'
import { envForRemoteOperation } from './environment'
import { createForEachRefParser } from './git-delimiter-parser'
import { IRemote } from '../../models/remote'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Create a new branch from the given start point.
 *
 * @param repository - The repository in which to create the new branch
 * @param name       - The name of the new branch
 * @param startPoint - A committish string that the new branch should be based
 *                     on, or undefined if the branch should be created based
 *                     off of the current state of HEAD
 */
export async function createBranch(
  repository: Repository,
  name: string,
  startPoint: string | null,
  noTrack?: boolean
): Promise<void> {
  return withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().createRepositoryBranch(repository.path, name, startPoint, noTrack ?? false),
    true
  )
}

export const getBranchNames = ({ path }: Repository): Promise<string[]> => {
  const parser = createForEachRefParser({ name: '%(refname:short)' })
  return git(['branch', ...parser.formatArgs], path, 'getBranchNames').then(x =>
    parser.parse(x.stdout).map(b => b.name)
  )
}

/** Rename the given branch to a new name. */
export async function renameBranch(
  repository: Repository,
  branch: Branch,
  newName: string,
  force?: boolean
): Promise<void> {
  if (branch.ref !== `refs/heads/${branch.name}`) throw new Error('Only local branches can be renamed')
  return withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().renameRepositoryBranch(repository.path, branch.name, newName, branch.tip.sha),
    true
  )
}

/**
 * Delete the branch locally.
 */
export async function deleteLocalBranch(
  repository: Repository,
  branchName: string,
  expectedTip?: string
): Promise<true> {
  if (expectedTip === undefined) throw new Error('A reviewed branch tip is required')
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().deleteRepositoryBranch(repository.path, branchName, expectedTip),
    true
  )
  return true
}

/**
 * Deletes a remote branch
 *
 * @param remoteName - the name of the remote to delete the branch from
 * @param remoteBranchName - the name of the branch on the remote
 */
export async function deleteRemoteBranch(
  repository: Repository,
  remote: IRemote,
  remoteBranchName: string
): Promise<true> {
  const args = ['push', '--', remote.name, `:${remoteBranchName}`]

  // If the user is not authenticated, the push is going to fail
  // Let this propagate and leave it to the caller to handle
  const result = await git(args, repository.path, 'deleteRemoteBranch', {
    env: await envForRemoteOperation(remote.url),
    expectedErrors: new Set<DugiteError>([DugiteError.BranchDeletionFailed]),
  })

  // It's possible that the delete failed because the ref has already
  // been deleted on the remote. If we identify that specific
  // error we can safely remove our remote ref which is what would
  // happen if the push didn't fail.
  if (result.gitError === DugiteError.BranchDeletionFailed) {
    const ref = `refs/remotes/${remote.name}/${remoteBranchName}`
    await deleteRef(repository, ref)
  }

  return true
}

/**
 * Finds branches that have a tip equal to the given committish
 *
 * @param repository within which to execute the command
 * @param commitish a sha, HEAD, etc that the branch(es) tip should be
 * @returns list branch names. null if an error is encountered
 */
export async function getBranchesPointedAt(
  repository: Repository,
  commitish: string
): Promise<Array<string> | null> {
  const read = () => backend().branchesPointingAt(repository.path, commitish)
  const branches = backend().transportKind === 'wry'
    ? await withRepositoryGitOperation(repository.path, 'read', read, true)
    : await read()
  return branches === null ? null : Array.from(branches)

}

/**
 * Gets all branches that have been merged into the given branch
 *
 * @param repository The repository in which to search
 * @param branchName The to be used as the base branch
 * @returns map of branch canonical refs paired to its sha
 */
export async function getMergedBranches(
  repository: Repository,
  branchName: string
): Promise<Map<string, string>> {
  const canonicalBranchRef = formatAsLocalRef(branchName)
  const read = () => backend().mergedRepositoryBranches(repository.path, branchName)
  const branches = backend().transportKind === 'wry'
    ? await withRepositoryGitOperation(repository.path, 'read', read, true)
    : await read()
  return new Map(
    branches
      .filter(branch => branch.ref !== canonicalBranchRef)
      .map(branch => [branch.ref, branch.oid])
  )

}
