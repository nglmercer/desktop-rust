import { WorkingDirectoryStatus } from '../models/status'
import { DiffSelectionType } from '../models/diff'
import { Repository } from '../models/repository'
import { stat } from 'fs/promises'
import { join } from 'path'
import { backend } from '../backend/client'
import { withRepositoryGitOperation } from './git/repository-operation'

const ReceiveLimit = 100 * 1024 * 1024 // 100 MiB

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

  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    let batch: string[] = []
    const review = async () => {
      if (!batch.length) return
      const paths = batch
      batch = []
      const read = () => backend().repositoryLargeFiles(repository.path, paths)
      const large = backend().transportKind === 'wry'
        ? await withRepositoryGitOperation(repository.path, 'read', read, true)
        : await read()
      paths.forEach((path, index) => { if (large[index]) fileNames.push(path) })
    }
    for (const file of includedFiles) {
      if (batch.length >= 1000 || new TextEncoder().encode(JSON.stringify({ paths: [...batch, file.path] })).length > 60_000) await review()
      batch.push(file.path)
    }
    await review()
    return fileNames
  }

  for (const file of includedFiles) {
    const filePath = join(repository.path, file.path)
    try {
      const fileStatus = await stat(filePath)
      const fileSizeBytes = fileStatus.size
      if (fileSizeBytes > ReceiveLimit) {
        fileNames.push(file.path)
      }
    } catch (error) {
      log.debug(`Unable to get the file size for ${filePath}`, error)
    }
  }

  return fileNames
}
