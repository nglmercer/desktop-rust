import { Repository } from '../../models/repository'
import { SubmoduleEntry } from '../../models/submodule'
import { IRemote } from '../../models/remote'
import { Progress } from '../../models/progress'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Update submodules after a git operation.
 *
 * @param repository - The repository in which to update submodules
 * @param remote - The remote for environment setup (can be null)
 * @param progressCallback - An optional function which will be invoked
 *                           with information about the current progress
 *                           of the submodule update operation.
 * @param progressKind - The kind of progress event ('checkout', 'pull', etc.)
 * @param title - The title to use for progress reporting
 * @param targetOrRemote - The target (for checkout) or remote name (for pull)
 * @param allowFileProtocol - Whether to allow file:// protocol for submodules
 */
export async function updateSubmodulesAfterOperation<T extends Progress>(
  repository: Repository,
  remote: IRemote | null,
  progressCallback: ((progress: T) => void) | undefined,
  progressKind: T['kind'],
  title: string,
  targetOrRemote: string,
  allowFileProtocol: boolean
): Promise<void> {
  const progress = (description: string, value: number) =>
    progressCallback?.({
      kind: progressKind,
      title,
      description,
      value,
      ...(progressKind === 'checkout'
        ? { target: targetOrRemote }
        : { remote: targetOrRemote }),
    } as T)
  progress('Updating submodules', 0)
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () =>
      backend().repositoryUpdateSubmodules(repository.path, allowFileProtocol),
    true
  )
  progress('Submodules updated', 1)
}

export async function listSubmodules(
  repository: Repository
): Promise<ReadonlyArray<SubmoduleEntry>> {
  const read = () => backend().repositorySubmoduleStatus(repository.path)
  const stdout = await withRepositoryGitOperation(
    repository.path,
    'read',
    read,
    true
  )
  return parseSubmoduleStatus(stdout)
}

function parseSubmoduleStatus(stdout: string): ReadonlyArray<SubmoduleEntry> {
  const submodules = new Array<SubmoduleEntry>()

  // entries are of the format:
  //  1eaabe34fc6f486367a176207420378f587d3b48 git (v2.16.0-rc0)
  //
  // first character:
  //   - " " if no change
  //   - "-" if the submodule is not initialized
  //   - "+" if the currently checked out submodule commit does not match the SHA-1 found in the index of the containing repository
  //   - "U" if the submodule has merge conflicts
  //
  // then the 40-character SHA represents the current commit
  //
  // then the path to the submodule
  //
  // then the output of `git describe` for the submodule in braces
  // we're not leveraging this in the app, so go and read the docs
  // about it if you want to learn more:
  //
  // https://git-scm.com/docs/git-describe
  const statusRe = /^.([^ ]+) (.+) \((.+?)\)$/gm

  for (const [, sha, path, describe] of stdout.matchAll(statusRe)) {
    submodules.push(new SubmoduleEntry(sha, path, describe))
  }

  return submodules
}
