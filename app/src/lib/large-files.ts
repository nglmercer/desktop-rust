import { WorkingDirectoryStatus } from '../models/status'
import { DiffSelectionType } from '../models/diff'
import { Repository } from '../models/repository'
import { backend } from '../backend/client'
import { withRepositoryGitOperation } from './git/repository-operation'

/**
 * Retrieve paths of working directory files that are larger than a given Megabyte size.
 *
 * @param repository        - The repository from which the base file directory will be retrieved.
 * @param workingDirectory  - The collection of changed files, from which the selected files will
 *                            be determined.
 * @param maximumSizeMB     - The size limit (in Megabytes) at which an exceeding file size will
 *                            result in it's path being retrieved.
 */
export async function getLargeFilePaths(
  repository: Repository,
  workingDirectory: WorkingDirectoryStatus
) {
  const fileNames = new Array<string>()
  const workingDirectoryFiles = workingDirectory.files
  const includedFiles = workingDirectoryFiles.filter(
    file => file.selection.getSelectionType() !== DiffSelectionType.None
  )

  let batch: string[] = []
  const review = async () => {
    if (!batch.length) return
    const paths = batch
    batch = []
    const read = () => backend().repositoryLargeFiles(repository.path, paths)
    const large = await withRepositoryGitOperation(
      repository.path,
      'read',
      read,
      true
    )
    paths.forEach((path, index) => {
      if (large[index]) fileNames.push(path)
    })
  }
  for (const file of includedFiles) {
    if (
      batch.length >= 1000 ||
      new TextEncoder().encode(JSON.stringify({ paths: [...batch, file.path] }))
        .length > 60_000
    )
      await review()
    batch.push(file.path)
  }
  await review()
  return fileNames
}
