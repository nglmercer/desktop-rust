import { Repository } from '../../models/repository'
import { Branch, BranchType, ITrackingBranch } from '../../models/branch'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/** Get all the branches. */
export async function getBranches(
  repository: Repository,
  ...prefixes: string[]
): Promise<ReadonlyArray<Branch>> {
  if (!prefixes || !prefixes.length) {
    prefixes = ['refs/heads', 'refs/remotes']
  }

  const read = () => backend().repositoryBranches(repository.path, prefixes)
  const mapBranches = (
    branches: ReadonlyArray<{
      readonly name: string
      readonly upstream: string | null
      readonly oid: string
      readonly ref: string
    }>
  ) =>
    branches.map(
      branch =>
        new Branch(
          branch.name,
          branch.upstream,
          { sha: branch.oid },
          branch.ref.startsWith('refs/heads')
            ? BranchType.Local
            : BranchType.Remote,
          branch.ref
        )
    )

  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(
        repository.path,
        'read',
        async () => mapBranches(await read()),
        true
      )
    : mapBranches(await read())
}

/**
 * Gets all branches that differ from their upstream (i.e. they're ahead,
 * behind or both), excluding the current branch.
 */
export async function getBranchesDifferingFromUpstream(
  repository: Repository
): Promise<ReadonlyArray<ITrackingBranch>> {
  const prefixes = ['refs/heads', 'refs/remotes']

  const read = () => backend().repositoryBranches(repository.path, prefixes)
  const mapBranches = (
    branches: ReadonlyArray<{
      readonly ref: string
      readonly oid: string
      readonly upstream_ref: string | null
      readonly current: boolean
    }>
  ) =>
    trackingBranches(
      branches.map(branch => ({
        fullName: branch.ref,
        sha: branch.oid,
        upstream: branch.upstream_ref ?? '',
        symref: '',
        head: branch.current ? '*' : ' ',
      }))
    )

  return backend().transportKind === 'wry'
    ? withRepositoryGitOperation(
        repository.path,
        'read',
        async () => mapBranches(await read()),
        true
      )
    : mapBranches(await read())
}

interface ITrackingRef {
  readonly fullName: string
  readonly sha: string
  readonly upstream: string
  readonly symref: string
  readonly head: string
}

function trackingBranches(
  refs: ReadonlyArray<ITrackingRef>
): ReadonlyArray<ITrackingBranch> {
  const localBranches = []
  const remoteBranchShas = new Map<string, string>()

  // First we need to collect the relevant info from the command output:
  // - For local branches with upstream: name, ref, SHA and the upstream.
  // - For remote branches we only need the sha (and the ref as key).
  for (const ref of refs) {
    if (ref.symref.length > 0 || ref.head === '*') {
      // Exclude symbolic refs and the current branch
      continue
    }

    if (ref.fullName.startsWith('refs/heads')) {
      if (ref.upstream.length === 0) {
        // Exclude local branches without upstream
        continue
      }

      localBranches.push({
        ref: ref.fullName,
        sha: ref.sha,
        upstream: ref.upstream,
      })
    } else {
      remoteBranchShas.set(ref.fullName, ref.sha)
    }
  }

  const eligibleBranches = new Array<ITrackingBranch>()

  // Compare the SHA of every local branch with the SHA of its upstream and
  // collect the names of local branches that differ from their upstream.
  for (const branch of localBranches) {
    const remoteSha = remoteBranchShas.get(branch.upstream)

    if (remoteSha !== undefined && remoteSha !== branch.sha) {
      eligibleBranches.push({
        ref: branch.ref,
        sha: branch.sha,
        upstreamRef: branch.upstream,
        upstreamSha: remoteSha,
      })
    }
  }

  return eligibleBranches
}
