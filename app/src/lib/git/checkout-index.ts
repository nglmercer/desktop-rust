import { git } from './core'
import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/** Prepare bounded literal-path batches before starting an index mutation. */
export function batchIndexPaths(paths: ReadonlyArray<string>): ReadonlyArray<ReadonlyArray<string>> {
  const batches: Array<Array<string>> = []
  let batch: Array<string> = []
  let size = 0
  const encoder = new TextEncoder()
  for (const path of paths) {
    const pathSize = encoder.encode(JSON.stringify(path)).length + 1
    if (pathSize > 50_000) {
      throw new Error('Index path exceeds the request limit')
    }
    if (batch.length === 4096 || size + pathSize > 50_000) {
      batches.push(batch)
      batch = []
      size = 0
    }
    batch.push(path)
    size += pathSize
  }
  if (batch.length > 0) batches.push(batch)
  return batches
}

/**
 * Forcefully updates the working directory with information from the index
 * for a given set of files.
 *
 * This method is essentially the same as running `git checkout -- files`
 * except by using `checkout-index` we can pass the files we want updated
 * on stdin, avoiding all issues with too long arguments.
 *
 * Note that this function will not yield errors for paths that don't
 * exist in the index (-q).
 *
 * @param repository The repository in which to update the working directory
 *                   with information from the index
 *
 * @param paths      The relative paths in the working directory to update
 *                   with information from the index.
 */
export async function checkoutIndex(
  repository: Repository,
  paths: ReadonlyArray<string>
) {
  if (!paths.length) {
    return
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    // Keep requests below the bridge limit while preserving the order of the
    // original selection. Prepare every batch before changing any files.
    const batches = batchIndexPaths(paths)
    await withRepositoryGitOperation(
      repository.path,
      'mutation',
      async () => {
        for (const selected of batches) {
          await backend().repositoryCheckoutIndex(repository.path, selected)
        }
      },
      true
    )
    return
  }

  const options = {
    successExitCodes: new Set([0, 1]),
    stdin: paths.join('\0'),
  }

  await git(
    ['checkout-index', '-f', '-u', '-q', '--stdin', '-z'],
    repository.path,
    'checkoutIndex',
    options
  )
}
