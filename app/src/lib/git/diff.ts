import * as Path from 'path'

import { getBlobContents } from './show'

import { Repository } from '../../models/repository'
import {
  WorkingDirectoryFileChange,
  FileChange,
  AppFileStatusKind,
  SubmoduleStatus,
  CommittedFileChange,
} from '../../models/status'
import {
  DiffType,
  IRawDiff,
  IDiff,
  IImageDiff,
  Image,
  LineEndingsChange,
  parseLineEndingText,
  ILargeTextDiff,
} from '../../models/diff'

import { DiffParser } from '../diff-parser'
import { getOldPathOrDefault } from '../get-old-path'
import { forceUnwrap } from '../fatal-error'
import { IChangesetData, parseRawLogWithNumstat } from './log'
import { backend } from '../../backend/client'
import { getMergeBase } from './merge'
import { enableImagePreviewsForDDSFiles } from '../feature-flag'
import { unstageAll } from './reset'
import { preparePartialStagingDiffs, stageFiles } from './update-index'
import { withRepositoryGitOperation } from './repository-operation'
const NullTreeSHA = '4b825dc642cb6eb9a060e54bf8d69288fbee4904'

/**
 * V8 has a limit on the size of string it can create (~256MB), and unless we want to
 * trigger an unhandled exception we need to do the encoding conversion by hand.
 *
 * This is a hard limit on how big a buffer can be and still be converted into
 * a string.
 */
const MaxDiffBufferSize = 70e6 // 70MB in decimal

// Parsing creates objects for individual lines. Bound their count before
// decoding and parsing, since a small patch can contain millions of lines.
const MaxDiffLineCount = 250_000

/**
 * Where `MaxDiffBufferSize` is a hard limit, this is a suggested limit. Diffs
 * bigger than this _could_ be displayed but it might cause some slowness.
 */
const MaxReasonableDiffSize = MaxDiffBufferSize / 16 // ~4.375MB in decimal

/**
 * The longest line length we should try to display. If a diff has a line longer
 * than this, we probably shouldn't attempt it
 */
const MaxCharactersPerLine = 5000

/**
 * Utility function to check whether parsing this buffer is going to cause
 * issues at runtime.
 *
 * @param buffer A buffer of binary text from a spawned process
 */
function isValidBuffer(buffer: Buffer) {
  if (buffer.length > MaxDiffBufferSize) return false
  let lines = 0
  for (const byte of buffer) {
    if (byte === 10 && ++lines > MaxDiffLineCount) return false
  }
  return true
}

/** Is the buffer too large for us to reasonably represent? */
function isBufferTooLarge(buffer: Buffer) {
  return buffer.length >= MaxReasonableDiffSize
}

/** Is the diff too large for us to reasonably represent? */
function isDiffTooLarge(diff: IRawDiff) {
  for (const hunk of diff.hunks) {
    for (const line of hunk.lines) {
      if (line.text.length > MaxCharactersPerLine) {
        return true
      }
    }
  }

  return false
}

/**
 *  Defining the list of known extensions we can render inside the app
 */
const imageFileExtensions = new Set([
  '.png',
  '.jpg',
  '.jpeg',
  '.gif',
  '.ico',
  '.webp',
  '.bmp',
  '.avif',
])

if (enableImagePreviewsForDDSFiles()) {
  imageFileExtensions.add('.dds')
}

/**
 * Render the difference between a file in the given commit and its parent
 *
 * @param commitish A commit SHA or some other identifier that ultimately dereferences
 *                  to a commit.
 */
export async function getCommitDiff(
  repository: Repository,
  file: FileChange,
  commitish: string,
  hideWhitespaceInDiff: boolean = false
): Promise<IDiff> {
  const oldPath =
    file.status.kind === AppFileStatusKind.Renamed ||
    file.status.kind === AppFileStatusKind.Copied
      ? file.status.oldPath
      : undefined
  const encoded = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererCommitDiff(
        repository.path,
        commitish,
        file.path,
        oldPath,
        hideWhitespaceInDiff
      ),
    true
  )
  if ('kind' in encoded) {
    if (file.status.submoduleStatus === undefined)
      return { kind: DiffType.Unrenderable }
    return buildDiff(
      Buffer.from(encoded.prefix),
      repository,
      file,
      commitish,
      commitish
    )
  }
  return buildDiff(
    Buffer.from(encoded.buffer, encoded.byteOffset, encoded.byteLength),
    repository,
    file,
    commitish,
    commitish
  )
}

/**
 * Render the diff between two branches with --merge-base for a file
 * (Show what would be the result of merge)
 */
export async function getBranchMergeBaseDiff(
  repository: Repository,
  file: FileChange,
  baseBranchName: string,
  comparisonBranchName: string,
  hideWhitespaceInDiff: boolean = false,
  latestCommit: string
): Promise<IDiff> {
  const oldPath =
    file.status.kind === AppFileStatusKind.Renamed ||
    file.status.kind === AppFileStatusKind.Copied
      ? file.status.oldPath
      : undefined
  const encoded = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererCompareDiff(
        repository.path,
        baseBranchName,
        comparisonBranchName,
        true,
        file.path,
        oldPath,
        hideWhitespaceInDiff
      ),
    true
  )
  if (encoded === null) throw new Error('Invalid comparison diff response')
  if ('kind' in encoded) {
    if (file.status.submoduleStatus === undefined)
      return { kind: DiffType.Unrenderable }
    return buildDiff(
      Buffer.from(encoded.prefix),
      repository,
      file,
      latestCommit,
      latestCommit
    )
  }
  return buildDiff(
    Buffer.from(encoded.buffer, encoded.byteOffset, encoded.byteLength),
    repository,
    file,
    latestCommit,
    latestCommit
  )
}

/**
 * Render the difference between two commits for a file
 *
 */
export async function getCommitRangeDiff(
  repository: Repository,
  file: FileChange,
  commits: ReadonlyArray<string>,
  hideWhitespaceInDiff: boolean = false,
  useNullTreeSHA: boolean = false
): Promise<IDiff> {
  if (commits.length === 0) {
    throw new Error('No commits to diff...')
  }

  const oldestCommit = useNullTreeSHA ? NullTreeSHA : commits[0]
  const oldestCommitRef = useNullTreeSHA ? NullTreeSHA : `${commits[0]}^`
  const latestCommit = commits.at(-1) ?? ''
  const oldPath =
    file.status.kind === AppFileStatusKind.Renamed ||
    file.status.kind === AppFileStatusKind.Copied
      ? file.status.oldPath
      : undefined
  const encoded = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererCompareDiff(
        repository.path,
        oldestCommitRef,
        latestCommit,
        false,
        file.path,
        oldPath,
        hideWhitespaceInDiff
      ),
    true
  )
  if (encoded === null) {
    if (!useNullTreeSHA)
      return getCommitRangeDiff(
        repository,
        file,
        commits,
        hideWhitespaceInDiff,
        true
      )
    throw new Error('Unable to compare the selected commits')
  }
  if ('kind' in encoded) {
    if (file.status.submoduleStatus === undefined)
      return { kind: DiffType.Unrenderable }
    return buildDiff(
      Buffer.from(encoded.prefix),
      repository,
      file,
      latestCommit,
      oldestCommit
    )
  }
  return buildDiff(
    Buffer.from(encoded.buffer, encoded.byteOffset, encoded.byteLength),
    repository,
    file,
    latestCommit,
    oldestCommit
  )
}

/**
 * Get the files that were changed for the merge base comparison of two branches.
 * (What would be the result of a merge)
 */
export async function getBranchMergeBaseChangedFiles(
  repository: Repository,
  baseBranchName: string,
  comparisonBranchName: string,
  latestComparisonBranchCommitRef: string
): Promise<IChangesetData | null> {
  const mergeBaseCommit = await getMergeBase(
    repository,
    baseBranchName,
    comparisonBranchName
  )
  if (mergeBaseCommit === null) {
    return null
  }

  const bytes = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererCompareChanges(
        repository.path,
        baseBranchName,
        comparisonBranchName,
        true
      ),
    true
  )
  if (bytes === null) throw new Error('Invalid merge-base comparison')
  return parseRawLogWithNumstat(
    Buffer.from(bytes).toString('utf8'),
    `${latestComparisonBranchCommitRef}`,
    mergeBaseCommit
  )
}

export async function getCommitRangeChangedFiles(
  repository: Repository,
  shas: ReadonlyArray<string>,
  useNullTreeSHA: boolean = false
): Promise<IChangesetData> {
  if (shas.length === 0) {
    throw new Error('No commits to diff...')
  }

  const oldestCommitRef = useNullTreeSHA ? NullTreeSHA : `${shas[0]}^`
  const latestCommitRef = shas.at(-1) ?? ''
  const bytes = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererCompareChanges(
        repository.path,
        oldestCommitRef,
        latestCommitRef,
        false
      ),
    true
  )
  if (bytes === null) {
    if (!useNullTreeSHA)
      return getCommitRangeChangedFiles(repository, shas, true)
    throw new Error('Invalid commit range comparison')
  }
  return parseRawLogWithNumstat(
    Buffer.from(bytes).toString('utf8'),
    latestCommitRef,
    oldestCommitRef
  )
}

/**
 * Render the diff for a file within the repository working directory. The file will be
 * compared against HEAD if it's tracked, if not it'll be compared to an empty file meaning
 * that all content in the file will be treated as additions.
 */
export async function getWorkingDirectoryDiff(
  repository: Repository,
  file: WorkingDirectoryFileChange,
  hideWhitespaceInDiff: boolean = false,
  options: { readonly noTextconv?: boolean } = {}
): Promise<IDiff> {
  const isSubmodule = file.status.submoduleStatus !== undefined
  const base =
    !isSubmodule &&
    (file.status.kind === AppFileStatusKind.New ||
      file.status.kind === AppFileStatusKind.Untracked)
      ? 'empty'
      : file.status.kind === AppFileStatusKind.Renamed
        ? 'index'
        : 'head'
  const result = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryRendererWorkingDiff(
        repository.path,
        file.path,
        base,
        hideWhitespaceInDiff,
        options.noTextconv === true
      ),
    true
  )
  return buildDiff(
    Buffer.from(result.stdout, 'base64'),
    repository,
    file,
    'HEAD',
    'HEAD',
    parseLineEndingsWarning(Buffer.from(result.stderr, 'base64'))
  )
}

/**
 * The result of {@link getResolutionDiff}: the computed diff together with the
 * exact old (base) and new (target) content strings it was generated from.
 */
export interface IResolutionDiff {
  readonly diff: IDiff
  /** The full content of the base (old) side the diff was generated from. */
  readonly oldContents: string
  /** The full content of the target (new) side the diff was generated from. */
  readonly newContents: string
}

/**
 * Compute a diff between the working-tree file and either Copilot's
 * resolved content string or the content from a specific merge index stage.
 *
 * The baseline is always the on-disk file (which still has conflict markers
 * during an active merge). This gives a consistent view across all three
 * resolution options (Copilot, current, incoming) — the user sees exactly
 * what each choice changes relative to the file's current state.
 *
 * Two calling conventions:
 *
 * 1. **Content mode** — pass a `content` string (e.g. Copilot's resolved
 *    text) to diff directly against the working-tree file.
 * 2. **Stage mode** — pass `stage: 'ours' | 'theirs'` to read from the
 *    merge index (`git show :2:<path>` or `git show :3:<path>`).
 *    These always refer to git's definition: `ours` = stage 2 (HEAD at
 *    merge time), `theirs` = stage 3 (the commit being merged in). Note
 *    that during a rebase, git swaps these — the upstream branch is "ours"
 *    and the rebased commit is "theirs". The caller is responsible for
 *    mapping user-facing labels to the correct git side.
 *
 * If the requested stage blob doesn't exist (e.g. file deleted on that
 * side in a modify/delete conflict), the target content is empty, showing
 * the on-disk content as entirely deleted.
 *
 * Uses `git diff --no-index` with temp files.
 *
 * Returns the computed diff alongside the exact old (base) and new (target)
 * content strings the diff was generated from. Callers can use these to feed
 * syntax highlighting and context expansion, since the diff sides don't
 * correspond to any addressable git revision.
 */
export async function getResolutionDiff(
  repository: Repository,
  filePath: string,
  options: { content: string } | { stage: 'ours' | 'theirs' },
  hideWhitespaceInDiff: boolean = false
): Promise<IResolutionDiff> {
  const result = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryResolutionDiff(
        repository.path,
        filePath,
        options,
        hideWhitespaceInDiff
      ),
    true
  )
  return buildResolutionDiff(
    Buffer.from(result.raw),
    result.oldContents,
    result.newContents
  )
}

function buildResolutionDiff(
  stdout: Buffer,
  oldContents: string,
  newContents: string
): IResolutionDiff {
  if (!isValidBuffer(stdout))
    return { diff: { kind: DiffType.Unrenderable }, oldContents, newContents }
  const diff = diffFromRawDiffOutput(stdout)
  return {
    diff: {
      kind: isDiffTooLarge(diff) ? DiffType.LargeText : DiffType.Text,
      text: diff.contents,
      hunks: diff.hunks,
      maxLineNumber: diff.maxLineNumber,
      hasHiddenBidiChars: diff.hasHiddenBidiChars,
    },
    oldContents,
    newContents,
  }
}

/**
 * Render the diff for a list of files within the repository working directory.
 * The files will be compared against HEAD if it's tracked, if not it'll be
 * compared to an empty file meaning that all content in the file will be
 * treated as additions.
 *
 * @param repository The repository to get the diff for
 * @param files The list of files to get the diff for
 * @param commitish The commitish to compare against, if not provided it will
 *                  default to HEAD. Mainly used to get a diff that includes
 *                  both staged changes and the changes in a commit. For example,
 *                  when the user is amending a commit and wants to generate
 *                  a commit message based on both the new changes and the
 *                  changes in the commit.
 */
export async function getFilesDiffText(
  repository: Repository,
  files: ReadonlyArray<WorkingDirectoryFileChange>,
  commitish?: string
): Promise<string> {
  if (
    commitish !== undefined &&
    !/^(?:[a-fA-F0-9]{40}|[a-fA-F0-9]{64})\^$/.test(commitish)
  ) {
    throw new Error('Native staged diffs require a commit object ID revision')
  }

  // Clear the staging area, our diffs reflect the difference between the
  // working directory and the last commit (if any) so our commits should
  // do the same thing.
  const partialDiffs = await preparePartialStagingDiffs(repository, files)
  await unstageAll(repository)
  await stageFiles(repository, files, partialDiffs)

  const bytes = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryReadSnapshot(
        repository.path,
        'renderer_staged_diff',
        {
          amend_head: commitish === undefined ? null : commitish.slice(0, -1),
        }
      ),
    true
  )
  await unstageAll(repository)
  if (bytes === null) throw new Error('Unable to read staged diff')
  const output = Buffer.from(bytes).toString('utf8')
  if (output.length > 10 * 1024 * 1024) {
    throw new Error('Diff is too large to render')
  }
  return output
}

async function getImageDiff(
  repository: Repository,
  file: FileChange,
  newestCommitish: string,
  oldestCommitish: string
): Promise<IImageDiff> {
  let current: Image | undefined = undefined
  let previous: Image | undefined = undefined

  // Are we looking at a file in the working directory or a file in a commit?
  if (file instanceof WorkingDirectoryFileChange) {
    // No idea what to do about this, a conflicted binary (presumably) file.
    // Ideally we'd show all three versions and let the user pick but that's
    // a bit out of scope for now.
    if (file.status.kind === AppFileStatusKind.Conflicted) {
      return { kind: DiffType.Image }
    }

    // Does it even exist in the working directory?
    if (file.status.kind !== AppFileStatusKind.Deleted) {
      current = await getWorkingDirectoryImage(repository, file)
    }

    if (
      file.status.kind !== AppFileStatusKind.New &&
      file.status.kind !== AppFileStatusKind.Untracked
    ) {
      // If we have file.oldPath that means it's a rename so we'll
      // look for that file.
      previous = await getBlobImage(
        repository,
        getOldPathOrDefault(file),
        'HEAD'
      )
    }
  } else {
    // File status can't be conflicted for a file in a commit
    if (file.status.kind !== AppFileStatusKind.Deleted) {
      current = await getBlobImage(repository, file.path, newestCommitish)
    }

    // File status can't be conflicted for a file in a commit
    if (
      file.status.kind !== AppFileStatusKind.New &&
      file.status.kind !== AppFileStatusKind.Untracked &&
      file.status.kind !== AppFileStatusKind.Deleted
    ) {
      // TODO: commitish^ won't work for the first commit
      //
      // If we have file.oldPath that means it's a rename so we'll
      // look for that file.
      previous = await getBlobImage(
        repository,
        getOldPathOrDefault(file),
        `${oldestCommitish}^`
      )
    }

    if (
      file instanceof CommittedFileChange &&
      file.status.kind === AppFileStatusKind.Deleted
    ) {
      previous = await getBlobImage(
        repository,
        getOldPathOrDefault(file),
        file.parentCommitish
      )
    }
  }

  return {
    kind: DiffType.Image,
    previous: previous,
    current: current,
  }
}

export async function convertDiff(
  repository: Repository,
  file: FileChange,
  diff: IRawDiff,
  newestCommitish: string,
  oldestCommitish: string,
  lineEndingsChange?: LineEndingsChange
): Promise<IDiff> {
  const extension = Path.extname(file.path).toLowerCase()

  if (diff.isBinary) {
    // some extension we don't know how to parse, never mind
    if (!imageFileExtensions.has(extension)) {
      return {
        kind: DiffType.Binary,
      }
    } else {
      return getImageDiff(repository, file, newestCommitish, oldestCommitish)
    }
  }

  return {
    kind: DiffType.Text,
    text: diff.contents,
    hunks: diff.hunks,
    lineEndingsChange,
    maxLineNumber: diff.maxLineNumber,
    hasHiddenBidiChars: diff.hasHiddenBidiChars,
  }
}

/**
 * Map a given file extension to the related data URL media type
 */
function getMediaType(extension: string) {
  if (extension === '.png') {
    return 'image/png'
  }
  if (extension === '.jpg' || extension === '.jpeg') {
    return 'image/jpg'
  }
  if (extension === '.gif') {
    return 'image/gif'
  }
  if (extension === '.ico') {
    return 'image/x-icon'
  }
  if (extension === '.webp') {
    return 'image/webp'
  }
  if (extension === '.bmp') {
    return 'image/bmp'
  }
  if (extension === '.avif') {
    return 'image/avif'
  }
  if (extension === '.dds') {
    return 'image/vnd-ms.dds'
  }

  // fallback value as per the spec
  return 'text/plain'
}

/**
 * `git diff` will write out messages about the line ending changes it knows
 * about to `stderr` - this rule here will catch this and also the to/from
 * changes based on what the user has configured.
 */
const lineEndingsChangeRegex =
  /', (CRLF|CR|LF) will be replaced by (CRLF|CR|LF) the .*/

/**
 * Utility function for inspecting the stderr output for the line endings
 * warning that Git may report.
 *
 * @param error A buffer of binary text from a spawned process
 */
function parseLineEndingsWarning(error: Buffer): LineEndingsChange | undefined {
  if (error.length === 0) {
    return undefined
  }

  const errorText = error.toString('utf-8')
  const match = lineEndingsChangeRegex.exec(errorText)
  if (match) {
    const from = parseLineEndingText(match[1])
    const to = parseLineEndingText(match[2])
    if (from && to) {
      return { from, to }
    }
  }

  return undefined
}

/**
 * Utility function used by get(Commit|WorkingDirectory)Diff.
 *
 * Parses the output from a diff-like command that uses `--path-with-raw`
 */
function diffFromRawDiffOutput(output: Buffer): IRawDiff {
  // for now we just assume the diff is UTF-8, but given we have the raw buffer
  // we can try and convert this into other encodings in the future
  const result = output.toString('utf-8')

  const pieces = result.split('\0')
  const parser = new DiffParser()
  return parser.parse(forceUnwrap(`Invalid diff output`, pieces.at(-1)))
}

async function buildSubmoduleDiff(
  buffer: Buffer,
  repository: Repository,
  file: FileChange,
  status: SubmoduleStatus
): Promise<IDiff> {
  const path = file.path
  const fullPath = Path.join(repository.path, path)
  const url = await backend().repositorySubmoduleDisplayUrl(
    repository.path,
    path
  )

  let oldSHA = null
  let newSHA = null

  if (
    status.commitChanged ||
    file.status.kind === AppFileStatusKind.New ||
    file.status.kind === AppFileStatusKind.Deleted
  ) {
    const diff = buffer.toString('utf-8')
    const baseRegex = 'Subproject commit ([^\\n-]+)(-dirty)?$'
    oldSHA = diff.match(new RegExp('-' + baseRegex, 'm'))?.[1] ?? null
    newSHA = diff.match(new RegExp('\\+' + baseRegex, 'm'))?.[1] ?? null
  }

  return {
    kind: DiffType.Submodule,
    fullPath,
    path,
    url,
    status,
    oldSHA,
    newSHA,
  }
}

async function buildDiff(
  buffer: Buffer,
  repository: Repository,
  file: FileChange,
  newestCommitish: string,
  oldestCommitish: string,
  lineEndingsChange?: LineEndingsChange
): Promise<IDiff> {
  if (file.status.submoduleStatus !== undefined) {
    return buildSubmoduleDiff(
      buffer,
      repository,
      file,
      file.status.submoduleStatus
    )
  }

  if (!isValidBuffer(buffer)) {
    // the buffer's diff is too large to be renderable in the UI
    return { kind: DiffType.Unrenderable }
  }

  const diff = diffFromRawDiffOutput(buffer)

  if (isBufferTooLarge(buffer) || isDiffTooLarge(diff)) {
    // we don't want to render by default
    // but we keep it as an option by
    // passing in text and hunks
    const largeTextDiff: ILargeTextDiff = {
      kind: DiffType.LargeText,
      text: diff.contents,
      hunks: diff.hunks,
      lineEndingsChange,
      maxLineNumber: diff.maxLineNumber,
      hasHiddenBidiChars: diff.hasHiddenBidiChars,
    }

    return largeTextDiff
  }

  return convertDiff(
    repository,
    file,
    diff,
    newestCommitish,
    oldestCommitish,
    lineEndingsChange
  )
}

/**
 * Retrieve the binary contents of a blob from the object database
 *
 * Returns an image object containing the base64 encoded string,
 * as <img> tags support the data URI scheme instead of
 * needing to reference a file:// URI
 *
 * https://en.wikipedia.org/wiki/Data_URI_scheme
 */
export async function getBlobImage(
  repository: Repository,
  path: string,
  commitish: string
): Promise<Image> {
  const extension = Path.extname(path).toLowerCase()
  const contents = await getBlobContents(repository, commitish, path)
  return new Image(
    Uint8Array.from(contents).buffer,
    contents.toString('base64'),
    getMediaType(extension),
    contents.length
  )
}
/**
 * Retrieve the binary contents of a blob from the working directory
 *
 * Returns an image object containing the base64 encoded string,
 * as <img> tags support the data URI scheme instead of
 * needing to reference a file:// URI
 *
 * https://en.wikipedia.org/wiki/Data_URI_scheme
 */
export async function getWorkingDirectoryImage(
  repository: Repository,
  file: FileChange
): Promise<Image> {
  const bytes = await withRepositoryGitOperation(
    repository.path,
    'read',
    () =>
      backend().repositoryReadSnapshot(
        repository.path,
        'renderer_working_file',
        { path: file.path }
      ),
    true
  )
  if (bytes === null) throw new Error('Working image is unavailable')
  const contents = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const rawContents =
    bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
      ? bytes.buffer
      : bytes.slice().buffer
  return new Image(
    rawContents,
    contents.toString('base64'),
    getMediaType(Path.extname(file.path).toLowerCase()),
    contents.length
  )
}
