import { git } from './core'
import type { Repository } from '../../models/repository'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'

/**
 * Create a new tag on the given target commit.
 *
 * @param repository        - The repository in which to create the new tag.
 * @param name              - The name of the new tag.
 * @param targetCommitSha   - The SHA of the commit where the new tag will live on.
 */
export async function createTag(
  repository: Repository,
  name: string,
  targetCommitSha: string
): Promise<void> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    return withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().mutateRepositoryTag(repository.path, {
          method: 'tag_create_annotated',
          name,
          target: targetCommitSha,
        }),
      true
    )
  }
  const args = ['tag', '-a', '-m', '', name, targetCommitSha]

  await git(args, repository.path, 'createTag')
}

/**
 * Delete a tag.
 *
 * @param repository        - The repository in which to create the new tag.
 * @param name              - The name of the tag to delete.
 */
export async function deleteTag(
  repository: Repository,
  name: string
): Promise<void> {
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    return withRepositoryGitOperation(
      repository.path,
      'mutation',
      () =>
        backend().mutateRepositoryTag(repository.path, {
          method: 'tag_delete',
          name,
        }),
      true
    )
  }
  const args = ['tag', '-d', name]

  await git(args, repository.path, 'deleteTag')
}

/**
 * Gets all the local tags. Returns a Map with the tag name and the commit it points to.
 *
 * @param repository    The repository in which to get all the tags from.
 */
export async function getAllTags(
  repository: Repository
): Promise<Map<string, string>> {
  let stdout: string
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    const read = () => backend().repositoryRendererTagRefs(repository.path)
    stdout =
      backend().transportKind === 'wry'
        ? await withRepositoryGitOperation(repository.path, 'read', read, true)
        : await read()
  } else {
    const tags = await git(
      ['show-ref', '--tags', '-d'],
      repository.path,
      'getAllTags',
      {
        successExitCodes: new Set([0, 1]),
      }
    )
    stdout = tags.stdout
  }

  const tagsArray: Array<[string, string]> = stdout
    .split('\n')
    .filter(line => line !== '')
    .map(line => {
      const [commitSha, rawTagName] = line.split(' ')

      // Normalize tag names by removing the leading ref/tags/ and the trailing ^{}.
      //
      // git show-ref returns two entries for annotated tags:
      // deadbeef refs/tags/annotated-tag
      // de510b99 refs/tags/annotated-tag^{}
      //
      // The first entry sha correspond to the blob object of the annotation, while the second
      // entry corresponds to the actual commit where the tag was created.
      // By normalizing the tag name we can make sure that the commit sha gets stored in the returned
      // Map of commits (since git will always print the entry with the commit sha at the end).
      const tagName = rawTagName
        .replace(/^refs\/tags\//, '')
        .replace(/\^\{\}$/, '')

      return [tagName, commitSha]
    })

  return new Map(tagsArray)
}
