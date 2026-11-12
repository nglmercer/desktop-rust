import { git } from './core'
import { GitError } from 'dugite'
import { Repository } from '../../models/repository'
import {
  Branch,
  BranchType,
  IBranchTip,
  ITrackingBranch,
} from '../../models/branch'
import { createForEachRefParser } from './git-delimiter-parser'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/** Get all the branches. */
export async function getBranches(
  repository: Repository,
  ...prefixes: string[]
): Promise<ReadonlyArray<Branch>> {
  const { formatArgs, parse } = createForEachRefParser({
    fullName: '%(refname)',
    shortName: '%(refname:short)',
    upstreamShortName: '%(upstream:short)',
    sha: '%(objectname)',
    symRef: '%(symref)',
  })

  if (!prefixes || !prefixes.length) {
    prefixes = ['refs/heads', 'refs/remotes']
  }

  if (backend().transportKind === 'ws') {
    const branches = await backend().repositoryBranches(
      repository.path,
      prefixes
    )
    return branches.map(
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
  }

  if (backend().transportKind === 'wry') {
    const migrated = await withRepositoryGitOperation(
      repository.path,
      'read',
      async () => {
        try {
          const branches = await backend().repositoryBranches(
            repository.path,
            prefixes
          )
          return branches.map(
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
        } catch (error) {
          log.warn(
            'Rust branch enumeration failed; retrying with the existing Git path',
            error
          )
          return undefined
        }
      },
      true
    )
    if (migrated !== undefined) return migrated
  }

  // TODO: use expectedErrors here to handle a specific error
  // see https://github.com/desktop/desktop/pull/5299#discussion_r206603442 for
  // discussion about what needs to change
  const result = await git(
    ['for-each-ref', ...formatArgs, ...prefixes],
    repository.path,
    'getBranches',
    { expectedErrors: new Set([GitError.NotAGitRepository]) }
  )

  if (result.gitError === GitError.NotAGitRepository) {
    return []
  }

  const branches = []

  for (const ref of parse(result.stdout)) {
    // excude symbolic refs from the branch list
    if (ref.symRef.length > 0) {
      continue
    }

    const tip: IBranchTip = { sha: ref.sha }

    const type = ref.fullName.startsWith('refs/heads')
      ? BranchType.Local
      : BranchType.Remote

    const upstream =
      ref.upstreamShortName.length > 0 ? ref.upstreamShortName : null

    branches.push(new Branch(ref.shortName, upstream, tip, type, ref.fullName))
  }

  return branches
}

/**
 * Gets all branches that differ from their upstream (i.e. they're ahead,
 * behind or both), excluding the current branch.
 * Useful to narrow down a list of branches that could potentially be fast
 * forwarded.
 *
 * @param repository Repository to get the branches from.
 */
export async function getBranchesDifferingFromUpstream(
  repository: Repository
): Promise<ReadonlyArray<ITrackingBranch>> {
  const { formatArgs, parse } = createForEachRefParser({
    fullName: '%(refname)',
    sha: '%(objectname)', // SHA
    upstream: '%(upstream)',
    symref: '%(symref)',
    head: '%(HEAD)',
  })

  const prefixes = ['refs/heads', 'refs/remotes']

  if (backend().transportKind === 'ws') {
    const branches = await backend().repositoryBranches(
      repository.path,
      prefixes
    )
    return trackingBranches(
      branches.map(branch => ({
        fullName: branch.ref,
        sha: branch.oid,
        upstream: branch.upstream_ref ?? '',
        symref: '',
        head: branch.current ? '*' : ' ',
      }))
    )
  }

  if (backend().transportKind === 'wry') {
    const migrated = await withRepositoryGitOperation(
      repository.path,
      'read',
      async () => {
        try {
          const branches = await backend().repositoryBranches(
            repository.path,
            prefixes
          )
          return trackingBranches(
            branches.map(branch => ({
              fullName: branch.ref,
              sha: branch.oid,
              upstream: branch.upstream_ref ?? '',
              symref: '',
              head: branch.current ? '*' : ' ',
            }))
          )
        } catch (error) {
          log.warn(
            'Rust branch tracking read failed; retrying with the existing Git path',
            error
          )
          return undefined
        }
      },
      true
    )
    if (migrated !== undefined) return migrated
  }

  const result = await git(
    ['for-each-ref', ...formatArgs, ...prefixes],
    repository.path,
    'getBranchesDifferingFromUpstream',
    { expectedErrors: new Set([GitError.NotAGitRepository]) }
  )

  if (result.gitError === GitError.NotAGitRepository) {
    return []
  }

  return trackingBranches(parse(result.stdout))
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
