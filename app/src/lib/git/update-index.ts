import { git } from './core'
import { Repository } from '../../models/repository'
import { DiffSelectionType, DiffType, ITextDiff, ILargeTextDiff } from '../../models/diff'
import { applyPatchToIndex } from './apply'
import { backend } from '../../backend/client'
import { withRepositoryGitOperation } from './repository-operation'
import { batchIndexPaths } from './checkout-index'
import { getWorkingDirectoryDiff } from './diff'
import {
  WorkingDirectoryFileChange,
  AppFileStatusKind,
} from '../../models/status'

interface IUpdateIndexOptions {
  /**
   * Whether or not to add a file when it exists in the working directory
   * but not in the index. Defaults to true (note that this differs from the
   * default behavior of Git which is to ignore new files).
   *
   * @default true
   */
  add?: boolean

  /**
   * Whether or not to remove a file when it exists in the index but not
   * in the working directory. Defaults to true (note that this differs from
   * the default behavior of Git which is to ignore removed files).
   *
   * @default true
   */
  remove?: boolean

  /**
   * Whether or not to forcefully remove a file from the index even though it
   * exists in the working directory. This implies remove.
   *
   * @default false
   */
  forceRemove?: boolean

  /**
   * Whether or not to replace conflicting entries in the index with that of
   * the working directory. Imagine the following scenario
   *
   * $ touch foo && git update-index --add foo && git commit -m 'foo'
   * $ rm foo && mkdir foo && echo "bar" > foo/bar
   * $ git update-index --add foo/bar
   * error: 'foo/bar' appears as both a file and as a directory
   * error: foo/bar: cannot add to the index - missing --add option?
   * fatal: Unable to process path foo/bar
   *
   * Replace ignores this conflict and overwrites the index with the
   * newly created directory, causing the original foo file to be deleted
   * in the index. This behavior matches what `git add` would do in a similar
   * scenario.
   *
   * @default true
   */
  replace?: boolean
}

/** Order partial renames before any separately selected source. */
function orderPartialSelections(files: ReadonlyArray<WorkingDirectoryFileChange>): Array<WorkingDirectoryFileChange> {
  const partial = files.filter(file => file.selection.getSelectionType() !== DiffSelectionType.All)
  const byPath = new Map(partial.map(file => [file.path, file]))
  const dependencies = new Map(partial.map(file => [file, 0]))
  for (const file of partial) {
    if (file.status.kind === AppFileStatusKind.Renamed) {
      const source = byPath.get(file.status.oldPath)
      if (source !== undefined) dependencies.set(source, (dependencies.get(source) ?? 0) + 1)
    }
  }
  const ready = partial.filter(file => dependencies.get(file) === 0)
  const ordered: Array<WorkingDirectoryFileChange> = []
  let cursor = 0
  while (cursor < ready.length) {
    const file = ready[cursor++]
    if (file === undefined) break
    ordered.push(file)
    if (file.status.kind === AppFileStatusKind.Renamed) {
      const source = byPath.get(file.status.oldPath)
      if (source !== undefined) {
        const remaining = (dependencies.get(source) ?? 0) - 1
        dependencies.set(source, remaining)
        if (remaining === 0) ready.push(source)
      }
    }
  }
  if (ordered.length !== partial.length) throw new Error('Overlapping renamed selections must be reviewed separately')
  return ordered
}

/** Retain renamed partial diffs before resetting the index that displays them. */
export async function preparePartialStagingDiffs(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  certified?: ReadonlyMap<string, ITextDiff | ILargeTextDiff>
): Promise<ReadonlyMap<string, ITextDiff | ILargeTextDiff> | undefined> {
  if (typeof window === 'undefined' || backend().transportKind === 'none') return certified
  orderPartialSelections(files)
  const diffs = new Map(certified)
  for (const file of files) {
    if (file.status.kind !== AppFileStatusKind.Renamed || file.selection.getSelectionType() !== DiffSelectionType.Partial || diffs.has(file.id)) continue
    const diff = await getWorkingDirectoryDiff(repository, file)
    if (diff.kind !== DiffType.Text && diff.kind !== DiffType.LargeText) throw new Error('The renamed file no longer has a selectable text diff')
    diffs.set(file.id, diff)
  }
  return diffs
}

/**
 * Updates the index with file contents from the working tree. This method
 * is a noop when no paths are provided.
 *
 * @param paths   A list of paths which are to be updated with file contents and
 *                status from the working directory.
 *
 * @param options See the IUpdateIndexOptions interface for more details.
 */
async function updateIndex(
  repository: Repository,
  paths: ReadonlyArray<string>,
  options: IUpdateIndexOptions = {}
) {
  if (paths.length === 0) {
    return
  }

  if (typeof window !== 'undefined' && backend().transportKind !== 'none' && options.add !== false && options.remove !== false && options.replace !== false) {
    const batches = batchIndexPaths(paths)
    await withRepositoryGitOperation(repository.path, 'mutation', async () => {
      for (const selected of batches) {
        await backend().repositoryUpdateIndex(repository.path, selected, options.forceRemove === true)
      }
    }, true)
    return
  }

  const args = ['update-index']

  if (options.add !== false) {
    args.push('--add')
  }

  if (options.remove !== false || options.forceRemove === true) {
    args.push('--remove')
  }

  if (options.forceRemove) {
    args.push('--force-remove')
  }

  if (options.replace !== false) {
    args.push('--replace')
  }

  args.push('-z', '--stdin')

  await git(args, repository.path, 'updateIndex', {
    stdin: paths.join('\0'),
  })
}

/**
 * Stage all the given files by either staging the entire path or by applying
 * a patch.
 *
 * Note that prior to stageFiles the index has been completely reset,
 * the job of this function is to set up the index in such a way that it
 * reflects what the user has selected in the app.
 *
 * Certified partial diffs preserve restored selections across awaited staging.
 */
export async function stageFiles(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  certifiedPartialDiffs?: ReadonlyMap<string, ITextDiff | ILargeTextDiff>
): Promise<void> {
  const partialDiffs = await preparePartialStagingDiffs(repository, files, certifiedPartialDiffs)
  const normal = []
  const oldRenamed = []
  const partial = []
  const deletedFiles = []
  const fullSelections = new Set(files.filter(file => file.selection.getSelectionType() === DiffSelectionType.All).map(file => file.path))

  for (const file of files) {
    if (file.selection.getSelectionType() === DiffSelectionType.All) {
      normal.push(file.path)
      if (file.status.kind === AppFileStatusKind.Renamed) {
        oldRenamed.push(file.status.oldPath)
      } else if (file.status.kind === AppFileStatusKind.Deleted) {
        deletedFiles.push(file.path)
      }
    } else {
      partial.push(file)
    }
  }

  const orderedPartial = typeof window !== 'undefined' && backend().transportKind !== 'none'
    ? orderPartialSelections(files)
    : partial

  // Staging files happens in three steps.
  //
  // In the first step we run through all of the renamed files, or
  // more specifically the source files (old) that were renamed and
  // forcefully remove them from the index. We do this in order to handle
  // the scenario where a file has been renamed and a new file has been
  // created in its original position. Think of it like this
  //
  // $ touch foo && git add foo && git commit -m 'foo'
  // $ git mv foo bar
  // $ echo "I'm a new foo" > foo
  //
  // Now we have a file which is of type Renamed that has its path set
  // to 'bar' and its oldPath set to 'foo'. But there's a new file called
  // foo in the repository. So if the user selects the 'foo -> bar' change
  // but not the new 'foo' file for inclusion in this commit we don't
  // want to add the new 'foo', we just want to recreate the move in the
  // index. We do this by forcefully removing the old path from the index
  // and then later (in step 2) stage the new file.
  await updateIndex(repository, oldRenamed, { forceRemove: true })

  // In the second step we update the index to match
  // the working directory in the case of new, modified, deleted,
  // and copied files as well as the destination paths for renamed
  // paths.
  await updateIndex(repository, normal)

  // This third step will only happen if we have files that have been marked
  // for deletion. This covers us for files that were blown away in the last
  // updateIndex call
  await updateIndex(repository, deletedFiles, { forceRemove: true })

  // Finally we run through all files that have partial selections.
  // Apply rename sources after their destinations when both are partial.
  for (const file of orderedPartial) {
    await applyPatchToIndex(
      repository,
      file,
      partialDiffs?.get(file.id),
      file.status.kind === AppFileStatusKind.Renamed && fullSelections.has(file.status.oldPath)
    )
  }
}
