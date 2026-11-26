import { lstatSync } from '../../../web/fs'
import {
  lstat,
  mkdir,
  readFile,
  readlink,
  realpath,
  rename,
  unlink,
  writeFile,
} from '../../../web/fs-promises'
import { dirname, isAbsolute, join, resolve, win32 } from 'path'
import { Repository } from '../../../models/repository'
import { IAssistedCommitHead } from '../../../models/assisted-commit'
import { AppFileStatusKind } from '../../../models/status'
import { isErrnoException } from '../../errno-exception'
import {
  git,
  IGitBufferExecutionOptions,
  IGitBufferResult,
  IGitStringExecutionOptions,
  IGitStringResult,
} from '../core'
import { IRefLock } from '../update-ref'
import {
  acquireOwnedFileLock,
  IOwnedFileLease,
  IOwnedFileLock,
  releaseOwnedFileLock,
} from '../owned-file-lock'
import { AssistedCommitError } from './error'
import {
  CapturedFileState,
  IAssistedCommitData,
  IIndexState,
  ITreeEntry,
} from './state'
import { backend } from '../../../backend/client'

interface IFileMetadata {
  readonly size: number
  readonly mode: number
  readonly dev: number | bigint
  readonly ino: number | bigint
  readonly mtimeMs: number
  readonly ctimeMs: number
  readonly atimeMs: number
  isFile(): boolean
  isDirectory(): boolean
  isSymbolicLink(): boolean
}

export function removeGitLineEnding(output: string): string {
  return output.replace(/\r?\n$/, '')
}

export async function readOptionalFile(path: string): Promise<Buffer | null> {
  try {
    return await readFile(path)
  } catch (error) {
    if (isErrnoException(error) && error.code === 'ENOENT') {
      return null
    }
    throw error
  }
}

async function optionalStat(path: string): Promise<IFileMetadata | null> {
  try {
    return (await lstat(path)) as IFileMetadata
  } catch (error) {
    if (isErrnoException(error) && error.code === 'ENOENT') {
      return null
    }
    throw error
  }
}

export async function readHeadContext(repository: Repository): Promise<{
  readonly ref: string | null
  readonly sha: string | null
  readonly gitDir: string
  readonly worktreeRoot: string
  readonly refFormat: string
}> {
  const context = await backend().repositoryAssistedCommitHead(repository.path)
  if (context.kind === 'invalid') {
    throw new AssistedCommitError('unsafe-selection', context.message)
  }
  return {
    ref: context.ref,
    sha: context.sha,
    gitDir: context.gitDir,
    worktreeRoot: context.worktreeRoot,
    refFormat: context.refFormat,
  }
}

export async function readHead(
  repository: Repository
): Promise<IAssistedCommitHead> {
  const { ref, sha } = await readHeadContext(repository)
  return Object.freeze({ ref, sha })
}

export async function readIndexState(path: string): Promise<IIndexState> {
  const stat = await optionalStat(path)
  if (stat === null) {
    return { bytes: null, mode: 0o666 }
  }
  if (!stat.isFile()) {
    throw new AssistedCommitError(
      'index-changed',
      'The real Git index is not a regular file'
    )
  }
  return {
    bytes: await readFile(path),
    mode: stat.mode & 0o777,
    timestamps: { atimeMs: stat.atimeMs, mtimeMs: stat.mtimeMs },
  }
}

export function indexStatesEqual(a: IIndexState, b: IIndexState): boolean {
  return (
    (a.bytes === null
      ? b.bytes === null
      : b.bytes !== null && a.bytes.equals(b.bytes)) &&
    (a.bytes === null || a.mode === b.mode)
  )
}

export async function verifyIndex(
  data: IAssistedCommitData,
  expected: IIndexState = data.originalIndex
): Promise<void> {
  if (!indexStatesEqual(expected, await readIndexState(data.indexPath))) {
    throw new AssistedCommitError(
      'index-changed',
      'The real Git index changed during the assisted commit run'
    )
  }
}

export async function verifyHead(
  data: IAssistedCommitData,
  expectedTip: string | null
): Promise<void> {
  const head = await readHeadContext(data.repository)
  const root = await realpath(data.repository.path)
  if (
    root !== data.snapshot.repositoryPath ||
    (await realpath(head.gitDir)) !== data.gitDirectory ||
    head.ref !== data.snapshot.originalHead.ref ||
    head.sha !== expectedTip
  ) {
    throw new AssistedCommitError(
      'repository-changed',
      'Repository, HEAD, or its full original ref changed during the run'
    )
  }
  const bytes = await readFile(join(data.gitDirectory, 'HEAD'))
  if (
    data.snapshot.originalHead.ref !== null &&
    !bytes.equals(data.originalHeadBytes)
  ) {
    throw new AssistedCommitError(
      'repository-changed',
      'The original symbolic HEAD changed during the run'
    )
  }
}

export function validateSelectedPath(path: string): void {
  const parts = path.split('/')
  if (
    path.length === 0 ||
    path.includes('\0') ||
    isAbsolute(path) ||
    win32.isAbsolute(path) ||
    (__WIN32__ && path.includes('\\')) ||
    parts.some(
      p =>
        p.length === 0 || p === '.' || p === '..' || p.toLowerCase() === '.git'
    )
  ) {
    throw new AssistedCommitError(
      'unsafe-selection',
      `Unsafe repository-relative selected path: ${JSON.stringify(path)}`
    )
  }
}

async function verifyPathParents(root: string, path: string): Promise<void> {
  const parts = path.split('/')
  let parent = root
  for (const part of parts.slice(0, -1)) {
    parent = join(parent, part)
    const stat = await optionalStat(parent)
    if (stat === null) {
      return
    }
    if (!stat.isDirectory()) {
      throw new AssistedCommitError(
        'selection-changed',
        `Selected path has a non-directory or symlink ancestor: ${JSON.stringify(
          path
        )}`
      )
    }
  }
}

function sameFileIdentity(
  before: IFileMetadata,
  after: IFileMetadata
): boolean {
  return (
    before.dev === after.dev &&
    before.ino === after.ino &&
    before.mode === after.mode &&
    before.size === after.size &&
    before.mtimeMs === after.mtimeMs &&
    before.ctimeMs === after.ctimeMs
  )
}

export async function readSelectedFileState(
  root: string,
  path: string
): Promise<CapturedFileState> {
  await verifyPathParents(root, path)
  const absolutePath = join(root, path)
  const before = await optionalStat(absolutePath)
  if (before === null) {
    return { kind: 'missing', mode: 0 }
  }
  const mode = before.mode & 0o111
  if (before.isDirectory()) {
    return { kind: 'directory', mode }
  }
  if (!before.isFile() && !before.isSymbolicLink()) {
    throw new AssistedCommitError(
      'unsafe-selection',
      `Selected path is not a regular file or symlink: ${JSON.stringify(path)}`
    )
  }
  const bytes = before.isSymbolicLink()
    ? await readlink(absolutePath, { encoding: 'buffer' })
    : await readFile(absolutePath)
  const after = await optionalStat(absolutePath)
  if (after === null || !sameFileIdentity(before, after)) {
    throw new AssistedCommitError(
      'selection-changed',
      `Selected file changed while being captured: ${JSON.stringify(path)}`
    )
  }
  return {
    kind: before.isSymbolicLink() ? 'symlink' : 'file',
    mode,
    bytes,
    dev: Number(after.dev),
    ino: Number(after.ino),
    mtimeMs: after.mtimeMs,
    ctimeMs: after.ctimeMs,
  }
}

function fileStatesEqual(a: CapturedFileState, b: CapturedFileState): boolean {
  return (
    a.kind === b.kind &&
    a.mode === b.mode &&
    (!('bytes' in a) ||
      ('bytes' in b &&
        a.bytes.equals(b.bytes) &&
        a.dev === b.dev &&
        a.ino === b.ino &&
        a.mtimeMs === b.mtimeMs &&
        a.ctimeMs === b.ctimeMs))
  )
}

export async function verifySelectedFiles(
  data: IAssistedCommitData
): Promise<void> {
  for (const file of data.files) {
    const current = await readSelectedFileState(
      data.snapshot.repositoryPath,
      file.file.path
    )
    if (!fileStatesEqual(file.state, current)) {
      throw new AssistedCommitError(
        'selection-changed',
        `Selected file changed during the run: ${JSON.stringify(
          file.file.path
        )}`
      )
    }
  }
  await Promise.all(
    data.files.map(async file => {
      const current = await optionalStat(
        join(data.snapshot.repositoryPath, file.file.path)
      )
      const expected = file.state
      const matches =
        expected.kind === 'missing'
          ? current === null
          : current !== null &&
            (current.mode & 0o111) === expected.mode &&
            (expected.kind === 'directory'
              ? current.isDirectory()
              : (expected.kind === 'file' || expected.kind === 'symlink') &&
                (expected.kind === 'symlink'
                  ? current.isSymbolicLink()
                  : current.isFile()) &&
                current.dev === expected.dev &&
                current.ino === expected.ino &&
                current.mtimeMs === expected.mtimeMs &&
                current.ctimeMs === expected.ctimeMs)
      if (!matches) {
        throw new AssistedCommitError(
          'selection-changed',
          `Selected file changed across verification: ${JSON.stringify(
            file.file.path
          )}`
        )
      }
    })
  )
  verifySelectedFileVersionsSync(data)
}

/** Non-yielding final backing fence after all awaited selected content/metadata reads. */
export function verifySelectedFileVersionsSync(
  data: IAssistedCommitData
): void {
  for (const file of data.files) {
    let current: IFileMetadata | null
    try {
      // eslint-disable-next-line no-sync
      current = lstatSync(
        join(data.snapshot.repositoryPath, file.file.path)
      ) as IFileMetadata
    } catch (error) {
      if (isErrnoException(error) && error.code === 'ENOENT') {
        current = null
      } else {
        throw error
      }
    }
    const expected = file.state
    const matches =
      expected.kind === 'missing'
        ? current === null
        : current !== null &&
          (current.mode & 0o111) === expected.mode &&
          (expected.kind === 'directory'
            ? current.isDirectory()
            : (expected.kind === 'file' || expected.kind === 'symlink') &&
              (expected.kind === 'symlink'
                ? current.isSymbolicLink()
                : current.isFile()) &&
              current.dev === expected.dev &&
              current.ino === expected.ino &&
              current.mtimeMs === expected.mtimeMs &&
              current.ctimeMs === expected.ctimeMs)
    if (!matches) {
      throw new AssistedCommitError(
        'selection-changed',
        `Selected file changed at final acceptance fence: ${JSON.stringify(
          file.file.path
        )}`
      )
    }
  }
}

/** Escape a value inside a quoted Git configuration entry. */
export function quoteConfig(value: string): string {
  return value
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
    .replace(/\n/g, '\\n')
    .replace(/\t/g, '\\t')
}

/**
 * Freeze the effective configuration, including conditional includes.
 *
 * A private HEAD preserves the original branch name for hooks. Flattening
 * includes prevents their gitdir conditions from being reevaluated against the
 * temporary metadata directory.
 */
export async function initializePrivateRepository(
  repository: Repository,
  root: string,
  directory: string,
  headBytes: Buffer,
  indexPath: string
): Promise<Record<string, string | undefined>> {
  const entries = await backend().repositoryAssistedCommitConfig(
    repository.path
  )
  let text = ''
  for (const { section, subsection, name, value } of entries) {
    text += `[${section}${
      subsection === null ? '' : ` "${quoteConfig(subsection)}"`
    }]\n`
    text += `\t${name}${value === null ? '' : ` = "${quoteConfig(value)}"`}\n`
  }
  text +=
    '[core]\n\tbare = false\n\tsplitIndex = false\n\tfsmonitor = false\n\tuntrackedCache = false\n'
  text += '[extensions]\n\tworktreeConfig = false\n'
  // The private ref view must never prune the original repository's objects.
  text +=
    '[gc]\n\tauto = 0\n\tautoDetach = false\n[maintenance]\n\tauto = false\n'

  const refs = await backend().repositoryAssistedCommitRefs(repository.path)
  const objects = await gitPath(repository, 'objects')
  const attributesPath = await gitPath(repository, 'info/attributes')
  const hooksPath = await gitPath(repository, 'hooks')
  text += `[core]\n\thooksPath = "${quoteConfig(hooksPath)}"\n`
  const attributes = await readOptionalFile(attributesPath)
  const shallow = await readOptionalFile(
    process.env.GIT_SHALLOW_FILE ?? (await gitPath(repository, 'shallow'))
  )
  await mkdir(join(directory, 'refs'), { recursive: true })
  await mkdir(join(directory, 'info'), { recursive: true })
  await writeFile(join(directory, 'config'), text, { mode: 0o600 })
  await writeFile(join(directory, 'HEAD'), headBytes, { mode: 0o600 })
  let packedRefs = ''
  for (const { objectId: sha, name: ref, target } of refs) {
    const perWorktree = [
      'refs/bisect/',
      'refs/worktree/',
      'refs/rewritten/',
    ].some(prefix => ref.startsWith(prefix))
    if (target === null && !perWorktree) {
      packedRefs += `${sha} ${ref}\n`
    } else {
      const refPath = join(directory, ref)
      await mkdir(dirname(refPath), { recursive: true })
      await writeFile(
        refPath,
        target === null ? `${sha}\n` : `ref: ${target}\n`,
        { mode: 0o600 }
      )
    }
  }
  await writeFile(join(directory, 'packed-refs'), packedRefs, { mode: 0o600 })
  await writeFile(join(directory, 'empty-config'), '', { mode: 0o600 })
  if (attributes !== null) {
    await writeFile(join(directory, 'info', 'attributes'), attributes, {
      mode: 0o600,
    })
  }
  if (shallow !== null) {
    await writeFile(join(directory, 'shallow'), shallow, { mode: 0o600 })
  }
  const environment: Record<string, string | undefined> = {
    GIT_DIR: directory,
    GIT_COMMON_DIR: directory,
    GIT_WORK_TREE: root,
    GIT_INDEX_FILE: indexPath,
    GIT_OBJECT_DIRECTORY: objects,
    GIT_CONFIG_GLOBAL: join(directory, 'empty-config'),
    GIT_CONFIG_SYSTEM: join(directory, 'empty-config'),
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG: undefined,
    GIT_CONFIG_COUNT: '0',
    GIT_CONFIG_PARAMETERS: '',
    GIT_LITERAL_PATHSPECS: '1',
    GIT_SHALLOW_FILE: join(directory, 'shallow'),
    GIT_NO_REPLACE_OBJECTS: '1',
    GIT_GRAFT_FILE: join(directory, 'info', 'grafts'),
  }
  const externalAttributes: Buffer[] = []
  for (const source of ['GIT_ATTR_SYSTEM', 'GIT_ATTR_GLOBAL']) {
    const result = await privateGit(
      { repository, environment },
      ['var', source],
      'assistedCommitAttributeSource',
      { successExitCodes: new Set([0, 1]) }
    )
    // Git reports a disabled or absent attribute source with empty exit 1.
    if (result.exitCode === 1) {
      if (result.stdout.length !== 0 || result.stderr.length !== 0) {
        throw new AssistedCommitError(
          'unsafe-selection',
          `Could not capture Git attribute source ${source}`,
          { cause: new Error(result.stderr || result.stdout) }
        )
      }
      continue
    }
    const sourcePath = removeGitLineEnding(result.stdout)
    if (sourcePath.length > 0) {
      const bytes = await readOptionalFile(resolve(root, sourcePath))
      if (bytes !== null) {
        externalAttributes.push(bytes, Buffer.from('\n'))
      }
    }
  }
  // Global rules follow system rules, preserving their per-attribute precedence.
  const externalAttributesPath = join(directory, 'external-attributes')
  await writeFile(externalAttributesPath, Buffer.concat(externalAttributes), {
    mode: 0o600,
  })
  text += `[core]\n\tattributesFile = "${quoteConfig(
    externalAttributesPath
  )}"\n`
  await writeFile(join(directory, 'config'), text, { mode: 0o600 })
  environment.GIT_ATTR_NOSYSTEM = '1'
  return environment
}

export function privateGit(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  args: string[],
  name: string,
  options?: IGitStringExecutionOptions
): Promise<IGitStringResult>
export function privateGit(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  args: string[],
  name: string,
  options: IGitBufferExecutionOptions
): Promise<IGitBufferResult>
export function privateGit(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  args: string[],
  name: string,
  options?: IGitStringExecutionOptions | IGitBufferExecutionOptions
): Promise<IGitStringResult | IGitBufferResult> {
  const env = { ...data.environment, ...options?.env }
  return options?.encoding === 'buffer'
    ? git(args, data.repository.path, name, {
        ...options,
        env,
        encoding: 'buffer',
      })
    : git(args, data.repository.path, name, { ...options, env })
}

export async function readTreeEntry(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  tree: string,
  path: string
): Promise<ITreeEntry | null> {
  const result = await privateGit(
    data,
    ['ls-tree', '-z', '--full-tree', tree, '--', path],
    'assistedCommitReadTreeEntry'
  )
  if (result.stdout.length === 0) {
    return null
  }
  const records = result.stdout.split('\0').filter(x => x.length !== 0)
  const tab = records[0].indexOf('\t')
  const [mode, type, sha] = records[0].slice(0, tab).split(' ')
  if (
    records.length !== 1 ||
    tab === -1 ||
    records[0].slice(tab + 1) !== path ||
    type !== 'blob'
  ) {
    throw new AssistedCommitError(
      'unsafe-selection',
      `Selected trees or submodules are not supported: ${JSON.stringify(path)}`
    )
  }
  return { path, mode, sha }
}

export async function readBlob(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  entry: ITreeEntry | null
): Promise<Buffer> {
  return entry === null
    ? Buffer.alloc(0)
    : (
        await privateGit(
          data,
          ['cat-file', 'blob', entry.sha],
          'assistedCommitReadBlob',
          {
            encoding: 'buffer',
          }
        )
      ).stdout
}

export async function hashBlob(
  data: Pick<IAssistedCommitData, 'repository' | 'environment'>,
  bytes: Buffer,
  path?: string
): Promise<string> {
  const result = await privateGit(
    data,
    [
      'hash-object',
      '-w',
      '--stdin',
      ...(path === undefined ? ['--no-filters'] : [`--path=${path}`]),
    ],
    'assistedCommitHashBlob',
    { stdin: bytes }
  )
  return removeGitLineEnding(result.stdout)
}

export async function writeTree(
  data: Pick<
    IAssistedCommitData,
    'repository' | 'environment' | 'temporaryDirectory'
  >,
  base: string,
  entries: ReadonlyArray<{
    readonly path: string
    readonly entry: ITreeEntry | null
  }>,
  indexPath: string
): Promise<string> {
  const env = { GIT_INDEX_FILE: indexPath }
  const configuration = [
    '-c',
    `core.hooksPath=${join(data.temporaryDirectory, 'no-hooks')}`,
  ]
  await privateGit(
    data,
    [...configuration, 'read-tree', base],
    'assistedCommitReadTree',
    { env }
  )
  const removals = entries
    .map(x => `0 ${'0'.repeat(base.length)}\t${x.path}\0`)
    .join('')
  const additions = entries
    .flatMap(x =>
      x.entry === null ? [] : [`${x.entry.mode} ${x.entry.sha}\t${x.path}\0`]
    )
    .join('')
  if (entries.length > 0) {
    await privateGit(
      data,
      [...configuration, 'update-index', '-z', '--index-info'],
      'assistedCommitWriteIndex',
      { env, stdin: removals + additions }
    )
  }
  return removeGitLineEnding(
    (
      await privateGit(
        data,
        [...configuration, 'write-tree'],
        'assistedCommitWriteTree',
        { env }
      )
    ).stdout
  )
}

export function fileTreeUpdates(file: {
  readonly file: {
    readonly path: string
    readonly status: {
      readonly kind: AppFileStatusKind
      readonly oldPath?: string
    }
  }
  readonly selected: ITreeEntry | null
}): ReadonlyArray<{
  readonly path: string
  readonly entry: ITreeEntry | null
}> {
  return [
    ...(file.file.status.kind === AppFileStatusKind.Renamed &&
    file.file.status.oldPath !== undefined
      ? [{ path: file.file.status.oldPath, entry: null }]
      : []),
    { path: file.file.path, entry: file.selected },
  ]
}

export function splitByteLines(bytes: Buffer): ReadonlyArray<Buffer> {
  const lines: Buffer[] = []
  let start = 0
  for (let i = 0; i < bytes.length; i++) {
    if (bytes[i] === 10) {
      lines.push(bytes.subarray(start, i + 1))
      start = i + 1
    }
  }
  if (start < bytes.length) {
    lines.push(bytes.subarray(start))
  }
  return lines
}

export type IOwnedIndexLock = IOwnedFileLock

export async function lockIndex(
  data: IAssistedCommitData,
  onOwnershipAcquired?: (lease: IOwnedFileLease) => void
): Promise<IOwnedIndexLock> {
  const path = `${data.indexPath}.lock`
  return acquireOwnedFileLock(
    path,
    data.originalIndex.mode,
    onOwnershipAcquired
  )
}

async function verifyOwnedLock(lock: IOwnedIndexLock): Promise<void> {
  const stat = await optionalStat(lock.path)
  if (
    stat === null ||
    stat.dev !== lock.stat.dev ||
    stat.ino !== lock.stat.ino
  ) {
    throw new AssistedCommitError(
      'index-changed',
      'The assisted commit index lock was removed or replaced by another writer'
    )
  }
}

export async function releaseIndexLock(lock: IOwnedIndexLock): Promise<void> {
  return releaseOwnedFileLock(lock)
}

export async function installIndex(
  data: IAssistedCommitData,
  lock: IOwnedIndexLock,
  expected: IIndexState,
  next: IIndexState,
  guard: IRefLock
): Promise<void> {
  guard.assertHeld()
  await verifyOwnedLock(lock)
  await verifyIndex(data, expected)
  guard.assertHeld()
  if (next.bytes === null) {
    await unlink(data.indexPath)
    guard.assertHeld()
    return
  }
  await lock.handle.writeFile(next.bytes)
  guard.assertHeld()
  await lock.handle.chmod(next.mode)
  if (next.timestamps !== undefined) {
    await lock.handle.utimes(
      next.timestamps.atimeMs / 1000,
      next.timestamps.mtimeMs / 1000
    )
  }
  await lock.handle.sync()
  guard.assertHeld()
  await verifyOwnedLock(lock)
  await verifyIndex(data, expected)
  guard.assertHeld()
  await lock.handle.close()
  lock.closed = true
  guard.assertHeld()
  await rename(lock.path, data.indexPath)
  lock.consumed = true
  guard.assertHeld()
}

export async function gitPath(
  repository: Repository,
  path: string
): Promise<string> {
  return backend().repositoryAssistedCommitGitPath(repository.path, path)
}

export async function ensureNoRepositoryOperation(
  repository: Repository
): Promise<void> {
  const safety = await backend().repositoryAssistedCommitSafety(repository.path)
  if (safety.kind === 'invalid') {
    throw new AssistedCommitError('unsafe-selection', safety.message)
  }
}
