import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Remove all files from the index
 *
 * @param repository the repository to update
 */
export async function unstageAllFiles(repository: Repository): Promise<void> {
  await withRepositoryGitOperation(
    repository.path,
    'mutation',
    () => backend().repositoryRemoveIndexAll(repository.path),
    true
  )
}
