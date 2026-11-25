import { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Retrieve the binary contents of a blob from the repository at a given
 * reference, commit, or tree.
 *
 * Returns a promise that will produce a Buffer instance containing
 * the binary contents of the blob or an error if the file doesn't
 * exists in the given revision.
 *
 * @param repository - The repository from where to read the blob
 *
 * @param commitish  - A commit SHA or some other identifier that
 *                     ultimately dereferences to a commit/tree.
 *
 * @param path       - The file path, relative to the repository
 *                     root from where to read the blob contents
 */
export const getBlobContents = async (
  repository: Repository,
  commitish: string,
  path: string
) => {
  const read = async () => {
    const snapshot = await backend().repositoryBlobSnapshot(
      repository.path,
      commitish,
      path
    )
    if (snapshot === null)
      throw new Error('Blob path does not exist in the requested revision')
    if (snapshot !== 'unsupported')
      return Buffer.from(
        snapshot.buffer,
        snapshot.byteOffset,
        snapshot.byteLength
      )
    const chunks: Buffer[] = []
    let offset = 0
    let oid: string | null = null
    let size: number | null = null
    do {
      const chunk = await backend().repositoryBlobChunk(
        repository.path,
        commitish,
        path,
        offset,
        1024 * 1024,
        oid
      )
      if (chunk === null)
        throw new Error('Blob path does not exist in the requested revision')
      if (size !== null && chunk.size !== size)
        throw new Error('Blob size changed during read')
      oid = chunk.oid
      size = chunk.size
      const bytes = Buffer.from(chunk.data, 'base64')
      chunks.push(bytes)
      offset += bytes.length
    } while (size !== null && offset < size)
    return Buffer.concat(chunks)
  }
  return withRepositoryGitOperation(repository.path, 'read', read, true)
}

/**
 * Retrieve some or all binary contents of a blob from the repository
 * at a given reference, commit, or tree. This is almost identical
 * to the getBlobContents method except that it supports only reading
 * a maximum number of bytes.
 *
 * Returns a promise that will produce a Buffer instance containing
 * the binary contents of the blob or an error if the file doesn't
 * exists in the given revision.
 *
 * @param repository - The repository from where to read the blob
 *
 * @param commitish  - A commit SHA or some other identifier that
 *                     ultimately dereferences to a commit/tree.
 *
 * @param path       - The file path, relative to the repository
 *                     root from where to read the blob contents
 *
 * @param length     - The maximum number of bytes to read from
 *                     the blob. Note that the number of bytes
 *                     returned may always be less than this number.
 */
export async function getPartialBlobContents(
  repository: Repository,
  commitish: string,
  path: string,
  length: number
): Promise<Buffer | null> {
  return getPartialBlobContentsCatchPathNotInRef(
    repository,
    commitish,
    path,
    length
  )
}

export async function getPartialBlobContentsCatchPathNotInRef(
  repository: Repository,
  commitish: string,
  path: string,
  length: number
): Promise<Buffer | null> {
  const read = () =>
    backend().repositoryBlobPrefix(repository.path, commitish, path, length)
  const result = await withRepositoryGitOperation(
    repository.path,
    'read',
    read,
    true
  )
  return result === null ? null : Buffer.from(result, 'base64')
}
