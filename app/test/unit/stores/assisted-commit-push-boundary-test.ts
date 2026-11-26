/* eslint-disable no-sync */
import assert from 'node:assert'
import {
  afterEach,
  before,
  beforeEach,
  describe,
  it,
  type TestContext,
} from 'node:test'
import { vi } from 'vitest'
import { backend, overrideBackendForTests } from '../../../src/backend/client'
import {
  createForEachRefParser,
  createLogParser,
} from '../../../src/lib/git/git-delimiter-parser'
import * as Dugite from 'dugite'
import * as FileSystem from 'fs/promises'
import * as FileSystemSync from 'fs'
import type { PathLike } from 'fs'
import { basename, join, resolve, sep } from 'path'
import {
  installNodeHostBridge,
  setNodeHostBridgeLstatHook,
  setNodeHostBridgeReadRegularVerifiedHook,
  uninstallNodeHostBridge,
} from '../../helpers/node-host-bridge'

let harness: typeof import('../../helpers/assisted-commit-run')
let fixtures: typeof import('../../helpers/assisted-commit')
let remotes: typeof import('../../helpers/assisted-commit-push')
let errors: typeof import('../../../src/lib/assisted-commit-run')
let repositories: typeof import('../../../src/models/repository')
let temporary: typeof import('../../helpers/temp')
let diffs: typeof import('../../../src/models/diff')
let selectionStates: typeof import('../../../src/lib/app-state')
let operations: typeof import('../../../src/lib/git/repository-operation')
let actualExec: typeof Dugite.exec
let capturePushes = false
const pushArguments: Array<ReadonlyArray<string>> = []
const allArguments: Array<ReadonlyArray<string>> = []
const allExecutions: Array<{
  readonly path: string
  readonly args: ReadonlyArray<string>
}> = []
let reportPushFailure: Error | undefined
let beforePush: ((path: string) => Promise<void>) | undefined
let afterPush: ((path: string) => Promise<void>) | undefined
let afterFollowUpFetch: (() => Promise<void>) | undefined
let afterHistoryRead: (() => Promise<void>) | undefined
let beforeImageRead: (() => Promise<void>) | undefined
let beforeAliasFenceCapture: (() => Promise<void>) | undefined
let aliasResolutionPath: string | undefined
let beforePublicationConfig: ((path: string) => Promise<void>) | undefined
let afterConfigurationRead: ((path: string) => Promise<void>) | undefined
let afterPinnedLookup: ((path: string) => Promise<void>) | undefined
let denySyncStat: ((path: PathLike) => boolean) | undefined
let syncFailure: Error | undefined
let statusFailure: Error | undefined
let historyFailure: Error | undefined
let configurationReadFailure:
  | { readonly path: string; readonly error: Error }
  | undefined
let directoryStatFailure: Error | undefined
let retainDirectoryStatFailure = false
let failedDirectory: string | undefined
let markerOpenFailure: Error | undefined
let stagingDirectoryCreations = 0
let afterStagingDirectoryCreation: ((path: string) => Promise<void>) | undefined
let afterMarkerUnlink: ((path: string) => Promise<void>) | undefined
let afterPublicationLockClose: ((path: string) => Promise<void>) | undefined
let afterModeStat: ((path: PathLike) => Promise<void>) | undefined
let afterFinalPublicationLockStat: ((path: string) => void) | undefined
const publicationLockVerifiedReads: string[] = []
let afterPublicationTrackingProbe: ((path: string) => Promise<void>) | undefined
let afterAlternateRoutingRead: ((path: string) => Promise<void>) | undefined
let denyUnlink: ((path: PathLike) => Promise<boolean>) | undefined
let denyRemove: ((path: PathLike) => Promise<boolean>) | undefined
let denyConfigKey: string | undefined
let duringConfigWrite:
  | ((args: ReadonlyArray<string>, path: string) => Promise<void>)
  | undefined

type InterceptedGitExec = (
  args: string[],
  path: string,
  options?: Dugite.IGitExecutionOptions
) => Promise<unknown>

/** Apply command-boundary faults to whichever host-backed Git executor is active. */
const interceptingGitExec = (
  execute: InterceptedGitExec
): InterceptedGitExec => {
  return async (args, path, options) => {
    if (
      historyFailure !== undefined &&
      pushArguments.length > 0 &&
      args.includes('log')
    ) {
      const failure = historyFailure
      historyFailure = undefined
      throw failure
    }
    if (
      statusFailure !== undefined &&
      pushArguments.length > 0 &&
      args.includes('status')
    ) {
      const failure = statusFailure
      statusFailure = undefined
      historyFailure = undefined
      throw failure
    }
    if (
      beforePublicationConfig !== undefined &&
      pushArguments.length > 0 &&
      args[0] === 'rev-parse' &&
      args.includes('--git-path') &&
      args.at(-1) === 'config'
    ) {
      await beforePublicationConfig(path)
    }
    if (
      duringConfigWrite !== undefined &&
      args[0] === 'config' &&
      (args.includes('--file') || args.includes('--local')) &&
      args.includes('--') &&
      (args.length > args.indexOf('--') + 2 ||
        (args.includes('--get-all') &&
          args[args.indexOf('--file') + 1] === '-'))
    ) {
      await duringConfigWrite(args, path)
    }
    if (capturePushes) {
      allArguments.push([...args])
      allExecutions.push({ path, args: [...args] })
    }
    if (
      denyConfigKey !== undefined &&
      args[0] === 'config' &&
      args.includes('--file') &&
      args[args.indexOf('--') + 1] === denyConfigKey
    ) {
      throw new Error('Synthetic publication metadata write failure')
    }
    if (capturePushes && args[0] === 'push') {
      pushArguments.push([...args])
      await beforePush?.(path)
      const result = await execute(args, path, options)
      await afterPush?.(path)
      if (reportPushFailure !== undefined) {
        const failure = reportPushFailure
        reportPushFailure = undefined
        throw failure
      }
      return result
    }
    const result = await execute(args, path, options)
    if (
      afterPublicationTrackingProbe !== undefined &&
      pushArguments.length > 0 &&
      args[0] === 'config' &&
      args.includes('--get-all') &&
      args.at(-1)?.endsWith('.merge')
    ) {
      const mutate = afterPublicationTrackingProbe
      afterPublicationTrackingProbe = undefined
      await mutate(path)
    }
    if (
      afterHistoryRead !== undefined &&
      pushArguments.length > 0 &&
      args.includes('log') &&
      args.includes('--numstat')
    ) {
      await afterHistoryRead()
    }
    if (
      afterFollowUpFetch !== undefined &&
      pushArguments.length > 0 &&
      args[0] === 'fetch' &&
      args.includes('--') &&
      args.at(-1) === 'origin'
    ) {
      await afterFollowUpFetch()
    }
    if (
      afterConfigurationRead !== undefined &&
      args[0] === 'config' &&
      args.includes('--list') &&
      args.includes('--show-origin')
    ) {
      await afterConfigurationRead(path)
    }
    if (
      afterPinnedLookup !== undefined &&
      args[0] === 'remote' &&
      args[1] === 'get-url' &&
      args.includes('--push') &&
      args.includes('--all') &&
      options?.env?.GIT_CONFIG_PARAMETERS?.includes('remote.origin.url=')
    ) {
      await afterPinnedLookup(path)
    }
    return result
  }
}

before(async () => {
  rustClient = backend()
  actualExec = Dugite.exec
  const unlink = FileSystem.unlink
  const remove = FileSystem.rm
  const stat = FileSystem.lstat
  const open = FileSystem.open
  const mkdtemp = FileSystem.mkdtemp
  const resolvePath = FileSystem.realpath
  const readFile = FileSystem.readFile
  const lstatSync = FileSystemSync.lstatSync
  const syncExports = {
    ...Object.fromEntries(
      Object.entries(FileSystemSync).filter(([name]) => name !== 'default')
    ),
    lstatSync: (...args: Parameters<typeof FileSystemSync.lstatSync>) => {
      if (denySyncStat?.(args[0])) {
        denySyncStat = undefined
        throw syncFailure ?? new Error('Synthetic metadata read failure')
      }
      const result = lstatSync(...args)
      if (
        typeof args[0] === 'string' &&
        basename(args[0]) === 'config.lock' &&
        afterFinalPublicationLockStat !== undefined
      ) {
        const mutate = afterFinalPublicationLockStat
        afterFinalPublicationLockStat = undefined
        mutate(args[0])
      }
      return result
    },
  }
  vi.doMock('fs', async () => ({
    default: syncExports,
    ...syncExports,
  }))
  vi.doMock('../../../src/web/fs.ts', async () => {
    const actual = await vi.importActual<typeof import('../../../src/web/fs')>(
      '../../../src/web/fs.ts'
    )
    return {
      ...actual,
      lstatSync: (...args: Parameters<typeof actual.lstatSync>) => {
        if (denySyncStat?.(args[0] as PathLike)) {
          denySyncStat = undefined
          throw syncFailure ?? new Error('Synthetic metadata read failure')
        }
        const result = actual.lstatSync(...args)
        if (
          typeof args[0] === 'string' &&
          basename(args[0]) === 'config.lock' &&
          afterFinalPublicationLockStat !== undefined
        ) {
          const mutate = afterFinalPublicationLockStat
          afterFinalPublicationLockStat = undefined
          mutate(args[0])
        }
        return result
      },
      readRegularFileVerifiedSync: (
        ...args: Parameters<typeof actual.readRegularFileVerifiedSync>
      ) => {
        if (
          typeof args[0] === 'string' &&
          basename(args[0]) === 'config.lock'
        ) {
          publicationLockVerifiedReads.push(args[0])
        }
        return actual.readRegularFileVerifiedSync(...args)
      },
    }
  })
  vi.doMock('dugite', async () => ({
    ...Dugite,
    exec: interceptingGitExec(actualExec),
  }))
  vi.doMock('../../../src/web/dugite.ts', async () => {
    const actual = await vi.importActual<
      typeof import('../../../src/web/dugite')
    >('../../../src/web/dugite.ts')
    return {
      ...actual,
      exec: interceptingGitExec(actual.exec as InterceptedGitExec),
    }
  })
  vi.doMock('fs/promises', async () => ({
    ...FileSystem,
    realpath: async (...args: Parameters<typeof FileSystem.realpath>) => {
      const result = await resolvePath(...args)
      if (args[0] === aliasResolutionPath) {
        await beforeAliasFenceCapture?.()
      }
      return result
    },
    readFile: async (...args: Parameters<typeof FileSystem.readFile>) => {
      if (typeof args[0] === 'string' && basename(args[0]) === 'image.png') {
        await beforeImageRead?.()
      }
      return readFile(...args)
    },
    open: async (...args: Parameters<typeof FileSystem.open>) => {
      if (
        markerOpenFailure !== undefined &&
        typeof args[0] === 'string' &&
        basename(args[0]) === '.desktop-publication-owner'
      ) {
        const error = markerOpenFailure
        markerOpenFailure = undefined
        throw error
      }
      const handle = await open(...args)
      if (
        afterAlternateRoutingRead !== undefined &&
        typeof args[0] === 'string' &&
        basename(args[0]) === 'alternates'
      ) {
        const path = args[0]
        const close = handle.close.bind(handle)
        handle.close = async () => {
          await close()
          await afterAlternateRoutingRead?.(path)
        }
      }
      if (
        afterPublicationLockClose !== undefined &&
        typeof args[0] === 'string' &&
        basename(args[0]) === 'config.lock' &&
        pushArguments.length > 0
      ) {
        const path = args[0]
        const close = handle.close.bind(handle)
        handle.close = async () => {
          await close()
          await afterPublicationLockClose?.(path)
        }
      }
      return handle
    },
    mkdtemp: async (...args: Parameters<typeof FileSystem.mkdtemp>) => {
      const directory = await mkdtemp(...args)
      if (
        typeof directory === 'string' &&
        basename(directory).startsWith('desktop-publication-')
      ) {
        stagingDirectoryCreations++
        await afterStagingDirectoryCreation?.(directory)
      }
      return directory
    },
    unlink: async (path: PathLike) => {
      if (await denyUnlink?.(path)) {
        throw Object.assign(
          new Error('Synthetic owned HEAD lock unlink failure'),
          {
            code: 'EACCES',
            errno: -13,
            syscall: 'unlink',
          }
        )
      }
      await unlink(path)
      if (
        typeof path === 'string' &&
        basename(path) === '.desktop-publication-owner'
      ) {
        await afterMarkerUnlink?.(path)
      }
    },
    rm: async (...args: Parameters<typeof FileSystem.rm>) => {
      if (await denyRemove?.(args[0])) {
        throw Object.assign(
          new Error('Synthetic publication directory cleanup EIO'),
          { code: 'EIO' }
        )
      }
      return remove(...args)
    },
    lstat: async (...args: Parameters<typeof FileSystem.lstat>) => {
      if (args[0] === configurationReadFailure?.path) {
        throw configurationReadFailure.error
      }
      if (
        directoryStatFailure !== undefined &&
        typeof args[0] === 'string' &&
        basename(args[0]).startsWith('desktop-publication-')
      ) {
        failedDirectory = args[0]
        const failure = directoryStatFailure
        if (!retainDirectoryStatFailure) {
          directoryStatFailure = undefined
        }
        throw failure
      }
      const result = await stat(...args)
      if (afterModeStat !== undefined && args[1] === undefined) {
        await afterModeStat(args[0])
      }
      return result
    },
  }))
  ;[
    harness,
    fixtures,
    remotes,
    errors,
    repositories,
    temporary,
    diffs,
    selectionStates,
    operations,
  ] = await Promise.all([
    import('../../helpers/assisted-commit-run'),
    import('../../helpers/assisted-commit'),
    import('../../helpers/assisted-commit-push'),
    import('../../../src/lib/assisted-commit-run'),
    import('../../../src/models/repository'),
    import('../../helpers/temp'),
    import('../../../src/models/diff'),
    import('../../../src/lib/app-state'),
    import('../../../src/lib/git/repository-operation'),
  ])
})

function reset() {
  capturePushes = false
  pushArguments.length = 0
  allArguments.length = 0
  allExecutions.length = 0
  reportPushFailure = undefined
  beforePush = undefined
  afterPush = undefined
  afterFollowUpFetch = undefined
  afterHistoryRead = undefined
  beforeImageRead = undefined
  beforeAliasFenceCapture = undefined
  aliasResolutionPath = undefined
  beforePublicationConfig = undefined
  afterConfigurationRead = undefined
  afterPinnedLookup = undefined
  denySyncStat = undefined
  syncFailure = undefined
  statusFailure = undefined
  configurationReadFailure = undefined
  directoryStatFailure = undefined
  retainDirectoryStatFailure = false
  failedDirectory = undefined
  markerOpenFailure = undefined
  stagingDirectoryCreations = 0
  afterStagingDirectoryCreation = undefined
  afterMarkerUnlink = undefined
  afterPublicationLockClose = undefined
  afterModeStat = undefined
  afterFinalPublicationLockStat = undefined
  publicationLockVerifiedReads.length = 0
  afterPublicationTrackingProbe = undefined
  afterAlternateRoutingRead = undefined
  denyUnlink = undefined
  denyRemove = undefined
  denyConfigKey = undefined
  duringConfigWrite = undefined
}

let rustClient: ReturnType<typeof backend>

function throwPendingHistoryFailure() {
  if (historyFailure !== undefined && pushArguments.length > 0) {
    const failure = historyFailure
    statusFailure = undefined
    historyFailure = undefined
    throw failure
  }
}

function parseDetailedStatusOutput(output: string) {
  const records = output.split('\0')
  const result: unknown[] = []
  const raw = (value: string) => ({ bytes: [...Buffer.from(value)] })
  for (let index = 0; index < records.length; index++) {
    const record = records[index]
    if (record.length === 0) continue
    if (record.startsWith('# ')) {
      result.push({ kind: 'header', value: raw(record.slice(2)) })
      continue
    }
    if (record.startsWith('? ')) {
      result.push({
        kind: 'entry',
        entry_kind: '?',
        status_code: '??',
        submodule_status_code: '????',
        path: raw(record.slice(2)),
        old_path: null,
        rename_or_copy_score: null,
      })
      continue
    }
    if (record.startsWith('! ')) continue
    const kind = record[0]
    const fields = kind === '1' ? 9 : kind === '2' ? 10 : 11
    const split = record.split(' ')
    const parts = [
      ...split.slice(0, fields - 1),
      split.slice(fields - 1).join(' '),
    ]
    const score = kind === '2' ? Number(parts[8].slice(1)) : null
    result.push({
      kind: 'entry',
      entry_kind: kind,
      status_code: parts[1],
      submodule_status_code: parts[2],
      path: raw(parts[fields - 1]),
      old_path: kind === '2' ? raw(records[++index]) : null,
      rename_or_copy_score: score,
    })
  }
  return result
}

/**
 * These cases inject filesystem, command-boundary, module-object, or Git env
 * faults that the separate Rust process cannot observe. Run only them through the renderer Node
 * path; the rest of the suite exercises the real Rust backend.
 */
function nodeBackend(t: TestContext) {
  overrideBackendForTests(
    new Proxy(
      {
        transportKind: 'none',
        pathExists: (path: string) => rustClient.pathExists(path),
        directoryExists: (path: string) => rustClient.directoryExists(path),
        repositoryType: (path: string) => rustClient.repositoryType(path),
        fetchRepositoryRemote: async (
          path: string,
          remote: string,
          expectedUrl: string,
          recurseSubmodules: boolean,
          isBackgroundTask = false,
          expectedSamlOwner?: string,
          onProgress?: never,
          refspec?: string,
          proxy?: never
        ) => {
          const result = await actualExec(
            [
              'fetch',
              '--progress',
              '--prune',
              recurseSubmodules
                ? '--recurse-submodules'
                : '--no-recurse-submodules',
              ...(refspec === undefined ? [] : [refspec]),
              '--',
              remote,
            ],
            path
          )
          if (pushArguments.length > 0) {
            await afterFollowUpFetch?.()
          }
          return result.exitCode === 0
            ? undefined
            : { exitCode: result.exitCode, errorKind: null }
        },
        pushRepositoryRemote: async (
          path: string,
          request: {
            remote: string
            assisted?: {
              expected_tip: string
              remote_ref: string
              config_parameters: string
            }
          },
          onProgress?: never,
          checkDestinations?: never,
          onHookProgress?: never,
          signal?: AbortSignal,
          onHookFailure?: never,
          onSpawnVerification?: () => Promise<void>
        ) => {
          if (request.assisted === undefined) {
            return rustClient.pushRepositoryRemote(
              path,
              request as never,
              onProgress,
              checkDestinations,
              onHookProgress,
              signal,
              onHookFailure,
              onSpawnVerification
            )
          }
          await onSpawnVerification?.()
          const result = (await interceptingGitExec(actualExec)(
            [
              'push',
              '--no-force',
              '--no-mirror',
              '--no-follow-tags',
              '--recurse-submodules=no',
              ...((request as { report_progress?: boolean }).report_progress
                ? ['--progress']
                : []),
              '--',
              request.remote,
              `${request.assisted.expected_tip}:${request.assisted.remote_ref}`,
            ],
            path,
            {
              env: {
                GIT_CONFIG: undefined,
                GIT_CONFIG_PARAMETERS: request.assisted.config_parameters,
              },
            } as never
          )) as Dugite.IGitStringResult
          return result.exitCode === 0
            ? undefined
            : { exitCode: result.exitCode, errorKind: null }
        },
        repositoryReadSnapshot: async (
          path: string,
          method: 'renderer_staged_diff' | 'renderer_working_file',
          params: Record<string, unknown>,
          expectedSha256?: string
        ) => {
          if (
            method === 'renderer_working_file' &&
            typeof params.path === 'string' &&
            basename(params.path) === 'image.png'
          ) {
            await beforeImageRead?.()
          }
          return rustClient.repositoryReadSnapshot(
            path,
            method,
            params,
            expectedSha256
          )
        },
        detailedRepositoryStatus: async (
          path: string,
          includeUntracked = true,
          options = {},
          creation = false
        ) => {
          if (statusFailure !== undefined && pushArguments.length > 0) {
            const failure = statusFailure
            statusFailure = undefined
            historyFailure = undefined
            throw failure
          }
          const result = await actualExec(
            [
              '--no-optional-locks',
              'status',
              '--branch',
              '--porcelain=2',
              '-z',
              ...(includeUntracked ? ['--untracked-files=all'] : []),
            ],
            path
          )
          return parseDetailedStatusOutput(result.stdout) as never
        },
        repositoryStatusContext: async (path: string) => {
          const gitDirResult = await actualExec(
            ['rev-parse', '--git-dir'],
            path
          )
          const gitDir = resolve(path, gitDirResult.stdout.trim())
          const exists = async (name: string) => {
            try {
              await FileSystem.lstat(join(gitDir, name))
              return true
            } catch {
              return false
            }
          }
          return {
            mergeHeadFound: await exists('MERGE_HEAD'),
            squashMsgFound: await exists('SQUASH_MSG'),
            isCherryPickingHeadFound: await exists('CHERRY_PICK_HEAD'),
            rebaseInternalState: null,
          }
        },
        repositoryAssistedCommitGitPath: async (
          path: string,
          gitPath: string
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['rev-parse', '--path-format=absolute', '--git-path', gitPath],
            path
          )) as Dugite.IGitStringResult
          return result.stdout.trim()
        },
        repositoryAssistedCommitTreeEntry: async (
          path: string,
          tree: string,
          entryPath: string,
          environment?: Record<string, string | undefined>
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['ls-tree', '-z', '--full-tree', tree, '--', entryPath],
            path,
            { encoding: 'buffer', env: environment } as never
          )) as Dugite.IGitBufferResult
          if (result.exitCode !== 0) {
            throw new Error('Cannot read assisted tree entry')
          }
          const stdout = Buffer.isBuffer(result.stdout)
            ? result.stdout
            : Buffer.from(result.stdout)
          const records: Buffer[] = []
          let start = 0
          for (let index = 0; index < stdout.length; index++) {
            if (stdout[index] === 0) {
              if (index > start) {
                records.push(stdout.subarray(start, index))
              }
              start = index + 1
            }
          }
          if (start < stdout.length) {
            records.push(stdout.subarray(start))
          }
          if (records.length === 0) {
            return { kind: 'missing' }
          }
          if (records.length !== 1) {
            return { kind: 'unsupported' }
          }
          const tab = records[0].indexOf(0x09)
          const fields = records[0]
            .subarray(0, tab === -1 ? 0 : tab)
            .toString('ascii')
            .split(' ')
          if (
            tab === -1 ||
            !records[0].subarray(tab + 1).equals(Buffer.from(entryPath)) ||
            fields.length !== 3 ||
            fields[1] !== 'blob'
          ) {
            return { kind: 'unsupported' }
          }
          return { kind: 'entry', mode: fields[0], sha: fields[2] }
        },
        repositoryAssistedCommitBlob: async (
          path: string,
          sha: string,
          environment?: Record<string, string | undefined>
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['cat-file', 'blob', sha],
            path,
            { encoding: 'buffer', env: environment } as never
          )) as Dugite.IGitBufferResult
          if (result.exitCode !== 0) {
            throw new Error('Cannot read assisted blob')
          }
          return Buffer.isBuffer(result.stdout)
            ? result.stdout
            : Buffer.from(result.stdout)
        },
        repositoryAssistedCommitCaptureConfig: async (
          path: string,
          environment?: Record<string, string | undefined>
        ) => {
          const readBoolean = async (key: string) => {
            const result = (await interceptingGitExec(actualExec)(
              ['config', '--bool', '--get', key],
              path,
              { successExitCodes: new Set([0, 1]), env: environment } as never
            )) as Dugite.IGitStringResult
            return result.exitCode === 1
              ? null
              : result.stdout.trim() === 'true'
          }
          const filters = (await interceptingGitExec(actualExec)(
            [
              'config',
              '--null',
              '--get-regexp',
              '^filter\\..*\\.(clean|process)$',
            ],
            path,
            { successExitCodes: new Set([0, 1]), env: environment } as never
          )) as Dugite.IGitStringResult
          return {
            filemode: await readBoolean('core.filemode'),
            symlinks: await readBoolean('core.symlinks'),
            filterOutput: filters.stdout,
          }
        },
        repositoryAssistedCommitDiffDriver: async (
          path: string,
          entryPath: string,
          environment?: Record<string, string | undefined>
        ) => {
          const run = async (
            args: string[],
            stdin?: string,
            successExitCodes?: Set<number>
          ) =>
            (await interceptingGitExec(actualExec)(args, path, {
              stdin,
              successExitCodes,
              env: environment,
            } as never)) as Dugite.IGitStringResult
          const attributes = await run(
            ['check-attr', '-z', '--stdin', 'diff'],
            `${entryPath}\0`
          )
          const attribute = attributes.stdout.split('\0')[2]
          const exists = async (driver: string) => {
            const result = await run(
              ['config', '--get', `diff.${driver}.textconv`],
              undefined,
              new Set([0, 1])
            )
            return result.exitCode === 0
          }
          if (
            attribute !== undefined &&
            ['set', 'unset', 'unspecified'].includes(attribute) &&
            (await exists(attribute))
          ) {
            return { kind: 'ambiguous-sentinel' }
          }
          const driver =
            attribute === undefined ||
            attribute === 'unspecified' ||
            attribute === 'set'
              ? 'default'
              : attribute
          if (await exists(driver)) {
            return { kind: 'textconv' }
          }
          if (driver !== 'default' && (await exists('default'))) {
            return { kind: 'fallback-textconv' }
          }
          return { kind: 'safe' }
        },
        repositoryAssistedCommitPushPaths: async (
          path: string,
          branchRef: string,
          environment?: Record<string, string | undefined>
        ) => {
          const run = async (args: string[]) => {
            const result = (await interceptingGitExec(actualExec)(args, path, {
              env: { GIT_CONFIG: undefined, ...environment },
            } as never)) as Dugite.IGitStringResult
            return result.stdout.replace(/\r?\n$/, '')
          }
          const gitPath = (name: string) => [
            'rev-parse',
            '--path-format=absolute',
            '--git-path',
            name,
          ]
          const [
            gitDirectory,
            config,
            commonDirectory,
            branch,
            packedRefs,
            shallow,
            grafts,
            replace,
          ] = await Promise.all([
            run(['rev-parse', '--absolute-git-dir']),
            run(gitPath('config')),
            run(['rev-parse', '--path-format=absolute', '--git-common-dir']),
            run(gitPath(branchRef)),
            run(gitPath('packed-refs')),
            run(gitPath('shallow')),
            run(gitPath('info/grafts')),
            run(gitPath('refs/replace')),
          ])
          return {
            gitDirectory,
            config,
            commonDirectory,
            branch,
            packedRefs,
            shallow,
            grafts,
            replace,
          }
        },
        repositoryAssistedCommitPushConfiguration: async (path: string) => {
          const result = (await interceptingGitExec(actualExec)(
            ['config', '--null', '--list', '--show-origin', '--includes'],
            path,
            { env: { GIT_CONFIG: undefined } } as never
          )) as Dugite.IGitStringResult
          const fields = result.stdout.split('\0').slice(0, -1)
          if (fields.length % 2 !== 0) {
            throw new Error('Git returned incomplete push configuration')
          }
          const entries: Array<{
            origin: string
            key: string
            value: string
          }> = []
          for (let index = 0; index < fields.length; index += 2) {
            const entry = fields[index + 1]
            const separator = entry.indexOf('\n')
            entries.push({
              origin: fields[index],
              key: separator === -1 ? entry : entry.slice(0, separator),
              value: separator === -1 ? '' : entry.slice(separator + 1),
            })
          }
          return entries
        },
        repositoryAssistedCommitConfigValues: async (
          path: string,
          key: string
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['config', '--null', '--get-all', '--', key],
            path,
            {
              successExitCodes: new Set([0, 1]),
              env: { GIT_CONFIG: undefined },
            } as never
          )) as Dugite.IGitStringResult
          return result.exitCode === 1
            ? []
            : result.stdout.split('\0').slice(0, -1)
        },
        repositoryAssistedCommitRemotePushUrls: async (
          path: string,
          remote: string,
          all: boolean,
          environment?: Record<string, string | undefined>
        ) => {
          const args = ['remote', 'get-url', '--push']
          if (all) args.push('--all')
          args.push('--', remote)
          const result = (await interceptingGitExec(actualExec)(args, path, {
            successExitCodes: new Set([0, 2]),
            env: { GIT_CONFIG: undefined, ...environment },
          } as never)) as Dugite.IGitStringResult
          return {
            exitCode: result.exitCode,
            stdout: result.stdout,
            stderr: result.stderr,
          }
        },
        repositoryAssistedCommitAttributeSource: async (
          path: string,
          variable: 'GIT_ATTR_SYSTEM' | 'GIT_ATTR_GLOBAL',
          environment?: Record<string, string | undefined>
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['var', variable],
            path,
            {
              successExitCodes: new Set([0, 1]),
              env: { GIT_CONFIG: undefined, ...environment },
            } as never
          )) as Dugite.IGitStringResult
          return {
            exitCode: result.exitCode,
            stdout: result.stdout,
            stderr: result.stderr,
          }
        },
        repositoryAssistedCommitConfigPath: async (
          path: string,
          variable: 'GIT_CONFIG_SYSTEM' | 'GIT_CONFIG_GLOBAL'
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['var', variable],
            path,
            { env: { GIT_CONFIG: undefined } } as never
          )) as Dugite.IGitStringResult
          return result.stdout
        },
        repositoryAssistedCommitRefFormat: async (
          path: string,
          reference: string
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            ['check-ref-format', reference],
            path,
            { successExitCodes: new Set([0, 1]) } as never
          )) as Dugite.IGitStringResult
          return result.exitCode === 0
        },
        repositoryAssistedCommitStagedConfigValues: async (
          path: string,
          key: string,
          input: Buffer,
          environment?: Record<string, string | undefined>
        ) => {
          const result = (await interceptingGitExec(actualExec)(
            [
              'config',
              '--no-includes',
              '--null',
              '--file',
              '-',
              '--get-all',
              '--',
              key,
            ],
            path,
            {
              stdin: input,
              env: { GIT_CONFIG: undefined, ...environment },
            } as never
          )) as Dugite.IGitStringResult
          return result.stdout.length === 0
            ? []
            : result.stdout.split('\0').slice(0, -1)
        },
        repositoryLastFetch: async (path: string) => {
          const gitDirResult = await actualExec(
            ['rev-parse', '--absolute-git-dir'],
            path
          )
          if (gitDirResult.exitCode !== 0) return { kind: 'unavailable' }
          try {
            const metadata = await FileSystem.stat(
              join(gitDirResult.stdout.trim(), 'FETCH_HEAD')
            )
            if (metadata.size === 0) return { kind: 'empty' }
            return { kind: 'fetched', time: Math.floor(metadata.mtimeMs) }
          } catch {
            return { kind: 'unavailable' }
          }
        },
        repositoryEffectiveGitIdentity: async (
          path: string,
          role: 'author' | 'committer'
        ) => {
          const result = await actualExec(
            [
              'var',
              role === 'author' ? 'GIT_AUTHOR_IDENT' : 'GIT_COMMITTER_IDENT',
            ],
            path,
            { successExitCodes: new Set([0, 128]) } as never
          )
          return result.exitCode === 0 ? result.stdout : null
        },
        repositoryWorktrees: async (path: string) => {
          const head = await actualExec(
            ['rev-parse', '--verify', '--quiet', 'HEAD'],
            path,
            { successExitCodes: new Set([0, 1]) } as never
          )
          const symbolic = await actualExec(
            ['symbolic-ref', '--quiet', '--short', 'HEAD'],
            path,
            { successExitCodes: new Set([0, 1]) } as never
          )
          return [
            {
              path,
              head: head.exitCode === 0 ? head.stdout.trim() : '0'.repeat(40),
              branch: symbolic.exitCode === 0 ? symbolic.stdout.trim() : null,
              isDetached: symbolic.exitCode !== 0,
              type: 'main',
              isLocked: false,
              isPrunable: false,
            },
          ]
        },
        repositoryGitReadSetting: async (
          path: string,
          key: string,
          onlyLocal: boolean,
          canonicalBool: boolean
        ) => {
          const result = await actualExec(
            [
              'config',
              ...(onlyLocal ? ['--local'] : []),
              ...(canonicalBool ? ['--type=bool'] : []),
              '--get',
              key,
            ],
            path,
            { successExitCodes: new Set([0, 1]) } as never
          )
          return result.exitCode === 0 ? result.stdout.trimEnd() : null
        },
        repositoryWorkingFileMetadataVersions: async (
          path: string,
          paths: ReadonlyArray<string>
        ) =>
          Promise.all(
            paths.map(async file => {
              try {
                const metadata = await FileSystem.lstat(join(path, file), {
                  bigint: true,
                })
                return [
                  metadata.dev,
                  metadata.ino,
                  metadata.mode,
                  metadata.size,
                  metadata.mtimeNs,
                  metadata.ctimeNs,
                ].join(':')
              } catch (error) {
                if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
                  return null
                }
                throw error
              }
            })
          ),
        repositoryAssistedCommitConfig: async (path: string) => {
          const result = await actualExec(
            ['config', '--null', '--list', '--includes'],
            path,
            {
              env: {
                GIT_CONFIG: undefined,
                GIT_CONFIG_COUNT: '0',
                GIT_CONFIG_PARAMETERS: '',
              },
            } as never
          )
          if (result.exitCode !== 0) {
            throw new Error('Cannot capture Git configuration')
          }
          return result.stdout
            .split('\0')
            .filter(item => item.length > 0)
            .flatMap(item => {
              const delimiter = item.indexOf('\n')
              const key = delimiter === -1 ? item : item.slice(0, delimiter)
              const value = delimiter === -1 ? null : item.slice(delimiter + 1)
              const firstDot = key.indexOf('.')
              const lastDot = key.lastIndexOf('.')
              if (firstDot === -1 || lastDot === -1) {
                throw new Error('configuration key has no section')
              }
              const section = key.slice(0, firstDot)
              if (
                section.toLowerCase() === 'include' ||
                section.toLowerCase() === 'includeif'
              ) {
                return []
              }
              return [
                {
                  section,
                  subsection:
                    firstDot === lastDot
                      ? null
                      : key.slice(firstDot + 1, lastDot),
                  name: key.slice(lastDot + 1),
                  value,
                },
              ]
            })
        },
        repositoryAssistedCommitRefs: async (path: string) => {
          const result = await actualExec(
            ['for-each-ref', '--format=%(objectname)%00%(refname)%00%(symref)'],
            path
          )
          if (result.exitCode !== 0) {
            throw new Error('Cannot capture repository references')
          }
          return result.stdout
            .replace(/\n$/, '')
            .split('\n')
            .filter(row => row.length > 0)
            .map(row => {
              const [objectId, name, target] = row.split('\0')
              return { objectId, name, target: target || null }
            })
        },
        repositoryAssistedCommitIndexEntries: async (path: string) => {
          const result = await actualExec(['ls-files', '--stage', '-z'], path, {
            encoding: 'buffer',
          } as never)
          if (result.exitCode !== 0) {
            throw new Error('Cannot capture index entries')
          }
          return Buffer.isBuffer(result.stdout)
            ? result.stdout
            : Buffer.from(result.stdout)
        },
        repositoryAssistedCommitSafety: async (path: string) => {
          const replacements = await actualExec(['replace', '--list'], path)
          if (replacements.exitCode !== 0) {
            throw new Error('Cannot inspect replacement refs')
          }
          if (replacements.stdout.length > 0) {
            return {
              kind: 'invalid',
              message:
                'Assisted commits do not support replacement refs or grafted history',
            }
          }
          const gitDirResult = await actualExec(
            ['rev-parse', '--absolute-git-dir'],
            path
          )
          const gitDir = gitDirResult.stdout.trim()
          const exists = async (name: string) => {
            try {
              await FileSystem.lstat(join(gitDir, name))
              return true
            } catch {
              return false
            }
          }
          if (await exists('info/grafts')) {
            return {
              kind: 'invalid',
              message:
                'Assisted commits do not support replacement refs or grafted history',
            }
          }
          for (const name of [
            'MERGE_HEAD',
            'CHERRY_PICK_HEAD',
            'REVERT_HEAD',
            'rebase-merge',
            'rebase-apply',
            'sequencer',
            'BISECT_LOG',
          ]) {
            if (await exists(name)) {
              return {
                kind: 'invalid',
                message: `Cannot create assisted commits during a Git operation (${name})`,
              }
            }
          }
          const conflicts = await actualExec(
            ['ls-files', '--unmerged', '-z'],
            path
          )
          if (conflicts.exitCode !== 0) {
            throw new Error('Cannot inspect unresolved index entries')
          }
          if (conflicts.stdout.length > 0) {
            return {
              kind: 'invalid',
              message: 'The real index contains unresolved conflicts',
            }
          }
          return { kind: 'ok' }
        },
        repositoryRendererWorkingDiff: async (
          path: string,
          file: string,
          base: 'head' | 'index' | 'empty',
          hideWhitespace: boolean,
          noTextconv: boolean
        ) => {
          const common = [
            'diff',
            ...(noTextconv ? ['--no-textconv'] : []),
            ...(hideWhitespace ? ['-w'] : []),
            '--no-ext-diff',
            '--patch-with-raw',
            '-z',
            '--no-color',
          ]
          if (base === 'empty') {
            let input: Buffer
            try {
              input = await FileSystem.readFile(join(path, file))
            } catch (error) {
              if ((error as NodeJS.ErrnoException).code === 'EISDIR') {
                return { stdout: '', stderr: '' }
              }
              throw error
            }
            const empty = await actualExec(
              [...common, '--no-index', '--', '/dev/null', '-'],
              path,
              {
                encoding: 'buffer',
                stdin: input,
                successExitCodes: new Set([0, 1]),
              } as never
            )
            const emptyBytes = (value: string | Buffer) =>
              Buffer.isBuffer(value) ? value : Buffer.from(value)
            return {
              stdout: emptyBytes(empty.stdout).toString('base64'),
              stderr: emptyBytes(empty.stderr).toString('base64'),
            }
          }
          const result = await actualExec(
            [...common, ...(base === 'head' ? ['HEAD'] : []), '--', file],
            path,
            { encoding: 'buffer' } as never
          )
          const bytes = (value: string | Buffer) =>
            Buffer.isBuffer(value) ? value : Buffer.from(value)
          return {
            stdout: bytes(result.stdout).toString('base64'),
            stderr: bytes(result.stderr).toString('base64'),
          }
        },
        repositorySafeFetchRemotes: async (path: string) => {
          const result = await actualExec(['remote', '-v'], path)
          if (result.exitCode !== 0) return null
          const remotes = new Map<string, string>()
          for (const line of result.stdout.split('\n')) {
            const match = /^([^\s]+)\s+([^\s]+) \(fetch\)$/.exec(line)
            if (match !== null) remotes.set(match[1], match[2])
          }
          return [...remotes].map(([name, url]) => ({ name, url }))
        },
        repositoryAssistedCommitHead: async (path: string) => {
          const run = async (args: string[]) => actualExec(args, path)
          const line = (value: string) => value.trimEnd()
          const directory = await run(['rev-parse', '--absolute-git-dir'])
          if (directory.exitCode !== 0) {
            return { kind: 'invalid', message: 'Cannot read the Git directory' }
          }
          const top = await run(['rev-parse', '--show-toplevel'])
          if (top.exitCode !== 0) {
            return { kind: 'invalid', message: 'Cannot read the worktree root' }
          }
          const format = await run(['rev-parse', '--show-ref-format'])
          if (format.exitCode !== 0) {
            return { kind: 'invalid', message: 'Cannot read the ref format' }
          }
          const symbolic = await run([
            'symbolic-ref',
            '--quiet',
            '--no-recurse',
            'HEAD',
          ])
          const ref = symbolic.exitCode === 0 ? line(symbolic.stdout) : null
          const tip = await run([
            'rev-parse',
            '--verify',
            '--quiet',
            '--end-of-options',
            'HEAD^{commit}',
          ])
          const sha = tip.exitCode === 0 ? line(tip.stdout) : null
          if (ref === null && sha === null) {
            return {
              kind: 'invalid',
              message: 'An unborn detached HEAD cannot be committed',
            }
          }
          return {
            kind: 'ok',
            gitDir: line(directory.stdout),
            worktreeRoot: line(top.stdout),
            refFormat: line(format.stdout),
            ref,
            sha,
          }
        },
        repositoryRefLockContext: async (
          path: string,
          reference: string,
          expectedValue: string
        ) => {
          const format = await actualExec(
            ['rev-parse', '--show-ref-format'],
            path
          )
          if (format.exitCode !== 0 || format.stdout.trim() !== 'files') {
            throw new Error(
              'Parent-owned ref locks require file-backed Git refs'
            )
          }
          const names = reference === 'HEAD' ? ['HEAD'] : ['HEAD', reference]
          const paths = []
          for (const name of names) {
            const output = await actualExec(
              ['rev-parse', '--path-format=absolute', '--git-path', name],
              path
            )
            if (output.exitCode !== 0) {
              throw new Error('Cannot resolve ref lock path')
            }
            paths.push(output.stdout.trim())
          }
          const current = await actualExec(
            [
              'rev-parse',
              '--verify',
              '--quiet',
              '--end-of-options',
              `${reference}^{commit}`,
            ],
            path,
            { successExitCodes: new Set([0, 1]) } as never
          )
          const actual =
            current.exitCode === 0
              ? current.stdout.trim()
              : '0'.repeat(expectedValue.length)
          return { paths, matches: actual === expectedValue }
        },
        repositoryProtectedResources: async (path: string) => {
          const gitOutput = async (args: string[]) => {
            const result = await actualExec(args, path)
            if (result.exitCode !== 0) {
              throw new Error(`Git resource discovery failed: ${result.stderr}`)
            }
            return result
          }
          const outputPath = (output: string) => output.replace(/\r?\n$/, '')
          const directory = outputPath(
            (await gitOutput(['rev-parse', '--absolute-git-dir'])).stdout
          )
          const common = outputPath(
            (
              await gitOutput([
                'rev-parse',
                '--path-format=absolute',
                '--git-common-dir',
              ])
            ).stdout
          )
          const object = outputPath(
            (
              await gitOutput([
                'rev-parse',
                '--path-format=absolute',
                '--git-path',
                'objects',
              ])
            ).stdout
          )
          const alternates = await gitOutput([
            '-c',
            'core.quotePath=false',
            'count-objects',
            '-v',
          ])
          const objects = [object]
          for (const line of alternates.stdout.split('\n')) {
            if (line.startsWith('alternate: ')) {
              objects.push(line.slice('alternate: '.length).replace(/\r$/, ''))
            }
          }
          const original = [path, directory, common, ...objects]
          const canonical = await Promise.all(
            original.map(async value => await FileSystem.realpath(value))
          )
          return {
            paths: [...original, ...canonical],
            objectDirectories: objects,
            objectRoutingDiagnostics: alternates.stderr
              .split('\n')
              .filter(
                line =>
                  line.length > 0 &&
                  !line.startsWith('warning: garbage found: ')
              )
              .join('\n'),
          }
        },
        canonicalMutationPath: (path: string) =>
          rustClient.canonicalMutationPath(path),
        repositoryRendererCommitDiff: async (
          path: string,
          revision: string,
          file: string,
          oldPath: string | undefined,
          hideWhitespace: boolean
        ) => {
          const result = await actualExec(
            [
              '--literal-pathspecs',
              'log',
              ...(hideWhitespace ? ['-w'] : []),
              '-m',
              '-1',
              '--first-parent',
              '--patch-with-raw',
              '--format=',
              '-z',
              '--no-color',
              '--end-of-options',
              revision,
              '--',
              file,
              ...(oldPath === undefined ? [] : [oldPath]),
            ],
            path,
            { encoding: 'buffer' } as never
          )
          if (result.exitCode !== 0) {
            throw new Error(result.stderr.toString())
          }
          return Buffer.isBuffer(result.stdout)
            ? new Uint8Array(result.stdout)
            : new Uint8Array(Buffer.from(result.stdout))
        },
        repositoryRendererHistory: async (
          path: string,
          revision: string | undefined,
          limit: number | undefined,
          skip: number | undefined,
          firstParent: boolean,
          excludeRemotes: boolean
        ) => {
          throwPendingHistoryFailure()
          const { formatArgs } = createLogParser({
            sha: '%H',
            shortSha: '%h',
            summary: '%s',
            body: '%b',
            author: '%an <%ae> %ad',
            committer: '%cn <%ce> %cd',
            parents: '%P',
            trailers: '%(trailers:unfold,only)',
            refs: '%D',
          })
          const result = await actualExec(
            [
              'log',
              '--date=raw',
              ...(limit === undefined || limit === null
                ? []
                : [`--max-count=${limit}`]),
              ...(skip === undefined || skip === null
                ? []
                : [`--skip=${skip}`]),
              ...formatArgs,
              '--no-show-signature',
              '--no-color',
              ...(firstParent ? ['--first-parent'] : []),
              ...(excludeRemotes ? ['--not', '--remotes'] : []),
              ...(revision === undefined
                ? []
                : [
                    ...(excludeRemotes ? ['--not'] : []),
                    '--end-of-options',
                    revision,
                  ]),
              '--',
            ],
            path,
            { successExitCodes: new Set([0, 128]) } as never
          )
          return result.exitCode === 128
            ? null
            : Buffer.from(result.stdout).toString('base64')
        },
        repositoryRendererCommit: async (path: string, ref: string) => {
          throwPendingHistoryFailure()
          await afterHistoryRead?.()
          const { formatArgs } = createLogParser({
            sha: '%H',
            shortSha: '%h',
            summary: '%s',
            body: '%b',
            author: '%an <%ae> %ad',
            committer: '%cn <%ce> %cd',
            parents: '%P',
            trailers: '%(trailers:unfold,only)',
            refs: '%D',
          })
          const result = await actualExec(
            [
              'log',
              '--date=raw',
              ...formatArgs,
              '--no-show-signature',
              '--no-color',
              '--end-of-options',
              ref,
              '--',
            ],
            path,
            { successExitCodes: new Set([0, 128]) } as any
          )
          return Buffer.from(result.stdout).toString('base64')
        },
        repositoryRendererCommitChanges: async (path: string, sha: string) => {
          throwPendingHistoryFailure()
          await afterHistoryRead?.()
          const result = await actualExec(
            [
              'log',
              sha,
              '-C',
              '-M',
              '-m',
              '-1',
              '--no-show-signature',
              '--first-parent',
              '--raw',
              '--format=format:',
              '--numstat',
              '-z',
              '--',
            ],
            path
          )
          return result.stdout
        },
        repositoryAuthors: async (
          path: string,
          revisions: ReadonlyArray<string>
        ) => {
          const result = await actualExec(
            [
              'log',
              '--format=%H%x00%an <%ae> %ad',
              '--no-walk=unsorted',
              '--date=raw',
              '-z',
              '--stdin',
            ],
            path,
            { stdin: revisions.join('\n') } as any
          )
          const parts = result.stdout.split('\0')
          const entries = []
          for (let i = 0; i + 1 < parts.length; i += 2) {
            entries.push({ oid: parts[i], identity: parts[i + 1] })
          }
          return entries
        },
        repositoryBranches: async (
          path: string,
          prefixes: ReadonlyArray<string>
        ) => {
          const { formatArgs, parse } = createForEachRefParser({
            fullName: '%(refname)',
            shortName: '%(refname:short)',
            upstreamShortName: '%(upstream:short)',
            upstream: '%(upstream)',
            sha: '%(objectname)',
            symref: '%(symref)',
            head: '%(HEAD)',
          })
          const result = await actualExec(
            ['for-each-ref', ...formatArgs, ...prefixes],
            path
          )
          return parse(result.stdout).map(ref => ({
            ref: ref.fullName,
            name: ref.shortName,
            upstream:
              ref.upstreamShortName.length > 0 ? ref.upstreamShortName : null,
            upstream_ref: ref.upstream.length > 0 ? ref.upstream : null,
            oid: ref.sha,
            current: ref.head === '*',
          }))
        },
        repositoryRendererTagRefs: async (path: string) => {
          const result = await actualExec(['show-ref', '--tags', '-d'], path, {
            successExitCodes: new Set([0, 1]),
          } as any)
          return result.stdout
        },
        repositorySymbolicRef: async (path: string, ref: string) => {
          const result = await actualExec(['symbolic-ref', '-q', ref], path, {
            successExitCodes: new Set([0, 1, 128]),
          } as any)
          return result.exitCode === 0 ? result.stdout.trim() : null
        },
        repositoryRendererReflog: async (path: string, kind: string) => {
          if (kind !== 'branches') return null
          const result = await actualExec(
            [
              'log',
              '-g',
              '--no-abbrev-commit',
              '--pretty=oneline',
              'HEAD',
              '-n',
              '2500',
              '--',
            ],
            path,
            { successExitCodes: new Set([0, 128]) } as any
          )
          return result.exitCode === 128 ? null : result.stdout
        },
        repositoryBranchCheckouts: async (path: string, afterDate: Date) => {
          const result = await actualExec(
            [
              'reflog',
              '--date=iso',
              `--after="${afterDate.toISOString()}"`,
              '--pretty=%H %gd %gs',
              '--grep-reflog=checkout: moving from .* to .*$',
              '--',
            ],
            path,
            { successExitCodes: new Set([0, 128]) } as any
          )
          return result.exitCode === 128 ? null : result.stdout
        },
      },
      {
        get(target, prop) {
          if (prop in target) return (target as any)[prop]
          const fallback = (rustClient as any)[prop]
          if (typeof fallback === 'function') {
            return fallback.bind(rustClient)
          }
          return (...args: unknown[]) => {
            console.error(
              'MISSING BACKEND METHOD',
              String(prop),
              JSON.stringify(args).slice(0, 200)
            )
            throw new Error(`missing backend method ${String(prop)}`)
          }
        },
      }
    ) as unknown as ReturnType<typeof backend>
  )
  t.after(() => overrideBackendForTests(rustClient))
}

describe('assisted native push boundary', () => {
  beforeEach(() => installNodeHostBridge())
  afterEach(() => uninstallNodeHostBridge())

  for (const empty of [false, true]) {
    it(`never publishes tracking into configuration covered by a foreign lease, empty ${empty}`, async t => {
      reset()
      t.after(reset)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      const foreign = await fixtures.seed(t, { foreign: 'Foreign checkout\n' })
      const sourceConfig = join(source.path, '.git', 'config')
      const foreignConfig = join(foreign.path, '.git', 'config')
      const configuration = await FileSystem.readFile(sourceConfig)
      await FileSystem.writeFile(foreignConfig, configuration)
      await FileSystem.unlink(sourceConfig)
      await FileSystem.symlink(foreignConfig, sourceConfig, 'file')
      if (!empty) {
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      }
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      h.dispatcher.updateCommitOptions(repository, { allowEmptyCommit: empty })
      const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      let releasedForTimeout = false
      const timer = setTimeout(() => {
        releasedForTimeout = true
        lease.release()
      }, 60000)
      capturePushes = true
      try {
        const request = await h.dispatcher.prepareCopilotAssistedCommitRequest(
          repository,
          h.request(repository)
        )
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          request
        )
        assert.deepStrictEqual(
          await FileSystem.readFile(foreignConfig),
          configuration
        )
        assert.ok(outcome.kind === 'push-error')
        assert.strictEqual(outcome.attempted, false)
        assert.strictEqual(
          await remote.read([
            'for-each-ref',
            '--format=%(objectname)',
            remote.remoteRef,
          ]),
          ''
        )
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(pushArguments.length, 0)
        assert.strictEqual(h.propose.mock.callCount(), empty ? 0 : 1)
        assert.strictEqual(releasedForTimeout, false)
      } finally {
        clearTimeout(timer)
        lease.release()
      }
    })
  }

  for (const input of ['global', 'system', 'include'] as const) {
    it(`certifies unresolved ${input} traversal without collapsing its native pathname`, async t => {
      reset()
      t.after(reset)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const directory = await temporary.createTempDirectory(t)
      const target = `${directory}${sep}absent-route${sep}..${sep}missing-config`
      if (input === 'include') {
        await fixtures.rawGit(source, [
          'config',
          '--local',
          '--',
          'includeIf.onbranch:another-layer5-branch.path',
          target,
        ])
      }
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const variable =
        input === 'system' ? 'GIT_CONFIG_SYSTEM' : 'GIT_CONFIG_GLOBAL'
      const previous = process.env[variable]
      const previousNoSystem = process.env.GIT_CONFIG_NOSYSTEM
      if (input !== 'include') {
        process.env[variable] = target
        process.env.GIT_CONFIG_NOSYSTEM = '0'
      }
      const failure = Object.assign(
        new Error('Uncollapsed configuration path EIO'),
        {
          code: 'EIO',
          syscall: 'lstat',
        }
      )
      configurationReadFailure = { path: target, error: failure }
      try {
        if (input === 'include') {
          await assert.rejects(
            h.dispatcher.prepareCopilotAssistedCommitRequest(
              repository,
              h.request(repository)
            ),
            error => errors.assistedCommitErrorCauses(error).includes(failure)
          )
          assert.strictEqual(await fixtures.count(source), 1)
          assert.strictEqual(h.propose.mock.callCount(), 0)
        } else {
          const outcome = await h.dispatcher.createCopilotAssistedCommits(
            repository,
            h.request(repository)
          )
          assert.ok(outcome.kind === 'push-error')
          assert.strictEqual(outcome.attempted, false)
          assert.strictEqual(pushArguments.length, 0)
          assert.strictEqual(await fixtures.count(source), 2)
          assert.strictEqual(h.propose.mock.callCount(), 1)
        }
        assert.strictEqual(await remote.tip(), remote.originalTip)
      } finally {
        if (previous === undefined) {
          delete process.env[variable]
        } else {
          process.env[variable] = previous
        }
        if (previousNoSystem === undefined) {
          delete process.env.GIT_CONFIG_NOSYSTEM
        } else {
          process.env.GIT_CONFIG_NOSYSTEM = previousNoSystem
        }
      }
    })
  }

  it('rejects configuration staging through a nonregular replacement after tracking probes and releases its lock', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const config = join(source.path, '.git', 'config')
    const original = await FileSystem.readFile(config)
    afterPublicationTrackingProbe = async () => {
      await FileSystem.rename(config, `${config}-original`)
      await FileSystem.mkdir(config)
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    await assert.rejects(FileSystem.lstat(`${config}.lock`), { code: 'ENOENT' })
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, null)
    assert.strictEqual(state.settling, false)
    await FileSystem.rmdir(config)
    await FileSystem.rename(`${config}-original`, config)
    assert.deepStrictEqual(await FileSystem.readFile(config), original)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
  })

  it('reads final publication bytes through a pinned host descriptor tied to the owned lock', async t => {
    reset()
    t.after(reset)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(outcome.refreshError, undefined)
    assert.ok(
      publicationLockVerifiedReads.some(
        path => basename(path) === 'config.lock'
      )
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
  })

  it('refuses a same-byte replacement between the final publication stat and descriptor open', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const config = join(source.path, '.git', 'config')
    const original = await FileSystem.readFile(config)
    let lockPath: string | undefined
    let replacement: Buffer | undefined
    setNodeHostBridgeReadRegularVerifiedHook(path => {
      if (basename(path) !== 'config.lock') {
        return
      }
      setNodeHostBridgeReadRegularVerifiedHook(undefined)
      lockPath = path
      replacement = FileSystemSync.readFileSync(path)
      const mode = FileSystemSync.statSync(path).mode & 0o777
      FileSystemSync.renameSync(path, `${path}-original`)
      FileSystemSync.writeFileSync(path, replacement, { mode })
    })
    t.after(() => setNodeHostBridgeReadRegularVerifiedHook(undefined))
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    assert.deepStrictEqual(await FileSystem.readFile(config), original)
    assert.ok(lockPath !== undefined && replacement !== undefined)
    assert.deepStrictEqual(await FileSystem.readFile(lockPath), replacement)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    await FileSystem.rename(lockPath, `${lockPath}-foreign`)
    await FileSystem.unlink(`${lockPath}-original`)
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    const recovered = h.state(repository).changesState.assistedCommit
    assert.strictEqual(
      recovered.kind,
      'idle',
      recovered.kind === 'error'
        ? errors
            .assistedCommitErrorCauses(recovered.error)
            .map(cause =>
              cause instanceof Error ? cause.message : String(cause)
            )
            .join('\n')
        : recovered.kind
    )
    assert.deepStrictEqual(
      await FileSystem.readFile(`${lockPath}-foreign`),
      replacement
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const empty of [false, true]) {
    it(`preserves local ${
      empty ? 'Empty' : 'selected'
    } execution when optional push configuration graph changes during capture`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const directory = await temporary.createTempDirectory(t)
      const globalConfiguration = join(directory, 'global-config')
      const included = join(directory, 'empty-include')
      await FileSystem.writeFile(globalConfiguration, '')
      await FileSystem.writeFile(included, '')
      if (!empty) {
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      }
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      await h.dispatcher.setCommitMessage(repository, {
        summary: 'Preserved manual draft',
        description: 'Preserved description',
        timestamp: 1,
      })
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      h.dispatcher.updateCommitOptions(repository, { allowEmptyCommit: empty })
      const previous = process.env.GIT_CONFIG_GLOBAL
      process.env.GIT_CONFIG_GLOBAL = globalConfiguration
      let changed = false
      afterConfigurationRead = async () => {
        afterConfigurationRead = undefined
        await FileSystem.appendFile(
          globalConfiguration,
          `[include]\n\tpath = "${included.replace(/\\/g, '\\\\')}"\n`
        )
        changed = true
      }
      capturePushes = true
      try {
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.strictEqual(
          changed,
          true,
          'The configuration graph mutation must execute'
        )
        assert.ok(outcome.kind === 'push-error', JSON.stringify(outcome))
        assert.strictEqual(outcome.attempted, false)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(await remote.tip(), remote.originalTip)
        assert.strictEqual(pushArguments.length, 0)
        assert.strictEqual(h.propose.mock.callCount(), empty ? 0 : 1)
        assert.strictEqual(h.commits.mock.callCount(), 1)
        assert.ok(
          errors
            .assistedCommitErrorCauses(outcome.error)
            .some(
              cause =>
                cause instanceof Error &&
                cause.message.includes('Configuration dependencies changed')
            )
        )
        if (empty) {
          assert.strictEqual(
            await fixtures.rawGit(source, [
              'show',
              '-s',
              '--format=%s',
              'HEAD',
            ]),
            'Empty commit'
          )
        }
        assert.strictEqual(
          h.state(repository).changesState.commitMessage.summary,
          'Preserved manual draft'
        )
      } finally {
        if (previous === undefined) {
          delete process.env.GIT_CONFIG_GLOBAL
        } else {
          process.env.GIT_CONFIG_GLOBAL = previous
        }
      }
    })
  }

  it('stops branch-conditional follow-up fetch after a successful hook changes symbolic HEAD', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await fixtures.rawGit(source, ['branch', '--', 'other', remote.originalTip])
    const tree = await fixtures.rawGit(source, ['rev-parse', 'HEAD^{tree}'])
    const unrelated = await fixtures.rawGit(source, [
      'commit-tree',
      tree,
      '-m',
      'Independent unrelated root',
    ])
    await fixtures.rawGit(source, [
      'update-ref',
      'refs/heads/unrelated',
      unrelated,
    ])
    const target = join(await temporary.createTempDirectory(t), 'branch-config')
    await FileSystem.writeFile(
      target,
      `[remote "origin"]\n\tfetch = +${remote.branchRef}:refs/heads/unrelated\n`
    )
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'includeIf.onbranch:other.path',
      target,
    ])
    await fixtures.writeHook(
      source,
      'pre-push',
      'git symbolic-ref HEAD refs/heads/other'
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'rev-parse',
        '--verify',
        'refs/heads/unrelated',
      ]),
      unrelated
    )
    assert.ok(outcome.refreshError)
    assert.strictEqual(
      await fixtures.rawGit(source, ['symbolic-ref', 'HEAD']),
      'refs/heads/other'
    )
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'rev-parse',
        '--verify',
        remote.branchRef,
      ]),
      outcome.result.head.sha
    )
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'rev-list',
        '--count',
        remote.branchRef,
        '--',
      ]),
      '2'
    )
    assert.strictEqual(
      await FileSystem.readFile(join(source.path, 'file'), 'utf8'),
      'after\n'
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('stops branch-conditional follow-up fetch after a successful hook introduces a symbolic branch alias', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await fixtures.rawGit(source, ['branch', '--', 'other', remote.originalTip])
    const tree = await fixtures.rawGit(source, ['rev-parse', 'HEAD^{tree}'])
    const unrelated = await fixtures.rawGit(source, [
      'commit-tree',
      tree,
      '-m',
      'Independent unrelated root',
    ])
    await fixtures.rawGit(source, [
      'update-ref',
      'refs/heads/unrelated',
      unrelated,
    ])
    const target = join(await temporary.createTempDirectory(t), 'branch-config')
    await FileSystem.writeFile(
      target,
      `[remote "origin"]\n\tfetch = +${remote.branchRef}:refs/heads/unrelated\n`
    )
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'includeIf.onbranch:other.path',
      target,
    ])
    const headPath = join(source.path, '.git', 'HEAD')
    const originalHead = await FileSystem.readFile(headPath)
    await fixtures.writeHook(
      source,
      'pre-push',
      `git symbolic-ref ${remote.branchRef} refs/heads/other`
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'rev-parse',
        '--verify',
        'refs/heads/unrelated',
      ]),
      unrelated
    )
    assert.ok(outcome.refreshError)
    assert.deepStrictEqual(await FileSystem.readFile(headPath), originalHead)
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'symbolic-ref',
        '--no-recurse',
        remote.branchRef,
      ]),
      'refs/heads/other'
    )
    assert.ok(outcome.result.head.sha !== null)
    assert.strictEqual(
      await fixtures.rawGit(source, [
        'rev-list',
        '--count',
        outcome.result.head.sha,
        '--',
      ]),
      '2'
    )
    assert.strictEqual(
      await FileSystem.readFile(join(source.path, 'file'), 'utf8'),
      'after\n'
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const empty of [false, true]) {
    it(`refuses same-root alternate-edge drift while freezing raw routes, empty ${empty}`, async t => {
      reset()
      t.after(reset)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const original = await fixtures.seed(t, {
        original: 'Original objects\n',
      })
      const foreign = await fixtures.seed(t, { foreign: 'Foreign objects\n' })
      await fixtures.rawGit(foreign, [
        'remote',
        'add',
        '--',
        'receiver',
        remote.path,
      ])
      await fixtures.rawGit(foreign, [
        'push',
        '--',
        'receiver',
        'HEAD:refs/heads/borrowed',
      ])
      const directory = await temporary.createTempDirectory(t)
      const oldAlias = join(directory, 'old-alternate')
      const newAlias = join(directory, 'new-alternate')
      for (const alias of [oldAlias, newAlias]) {
        await FileSystem.symlink(
          join(original.path, '.git', 'objects'),
          alias,
          'junction'
        )
      }
      const info = join(source.path, '.git', 'objects', 'info')
      await FileSystem.mkdir(info, { recursive: true })
      const routingFile = join(info, 'alternates')
      await FileSystem.writeFile(routingFile, `${oldAlias}\n`)
      if (!empty) {
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      }
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      await h.dispatcher.setCommitMessage(repository, {
        summary: 'Preserved manual draft',
        description: 'Preserved manual details',
        timestamp: Date.now(),
      })
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      h.dispatcher.updateCommitOptions(repository, { allowEmptyCommit: empty })
      const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      let releasedForTimeout = false
      const timer = setTimeout(() => {
        releasedForTimeout = true
        lease.release()
      }, 60000)
      let changed = false
      afterAlternateRoutingRead = async path => {
        if (
          (await FileSystem.realpath(path)) !==
          (await FileSystem.realpath(routingFile))
        ) {
          return
        }
        afterAlternateRoutingRead = undefined
        await FileSystem.writeFile(routingFile, `${newAlias}\n`)
        changed = true
      }
      afterPush = async () => {
        afterPush = undefined
        await FileSystem.unlink(newAlias)
        await FileSystem.symlink(
          join(foreign.path, '.git', 'objects'),
          newAlias,
          'junction'
        )
      }
      capturePushes = true
      try {
        const request = await h.dispatcher.prepareCopilotAssistedCommitRequest(
          repository,
          h.request(repository)
        )
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          request
        )
        assert.strictEqual(changed, true)
        assert.strictEqual(
          await fixtures.rawGit(source, [
            'for-each-ref',
            '--format=%(objectname)',
            'refs/remotes/origin/borrowed',
          ]),
          ''
        )
        assert.ok(outcome.kind === 'push-error')
        assert.strictEqual(outcome.attempted, false)
        assert.strictEqual(releasedForTimeout, false)
        assert.strictEqual(await remote.tip(), remote.originalTip)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(pushArguments.length, 0)
        assert.strictEqual(h.propose.mock.callCount(), empty ? 0 : 1)
        if (empty) {
          assert.strictEqual(
            await fixtures.rawGit(source, [
              'show',
              '-s',
              '--format=%s',
              'HEAD',
            ]),
            'Empty commit'
          )
        }
        assert.strictEqual(
          h.state(repository).changesState.commitMessage.summary,
          'Preserved manual draft'
        )
      } finally {
        clearTimeout(timer)
        lease.release()
      }
    })
  }

  for (const route of ['environment', 'file', 'transitive'] as const) {
    it(`refuses a retargeted raw ${route} alternate alias after acknowledgement`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const original = await fixtures.seed(t, {
        original: 'Original objects\n',
      })
      const intermediate = await fixtures.seed(t, {
        intermediate: 'Intermediate objects\n',
      })
      const foreign = await fixtures.seed(t, { foreign: 'Foreign objects\n' })
      await fixtures.rawGit(foreign, [
        'remote',
        'add',
        '--',
        'receiver',
        remote.path,
      ])
      await fixtures.rawGit(foreign, [
        'push',
        '--',
        'receiver',
        'HEAD:refs/heads/borrowed',
      ])
      const alias = join(await temporary.createTempDirectory(t), 'alternate')
      await FileSystem.symlink(
        join(original.path, '.git', 'objects'),
        alias,
        'junction'
      )
      const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      const previous = process.env.GIT_ALTERNATE_OBJECT_DIRECTORIES
      let releasedForTimeout = false
      const timer = setTimeout(() => {
        releasedForTimeout = true
        lease.release()
      }, 60000)
      try {
        if (route === 'environment') {
          process.env.GIT_ALTERNATE_OBJECT_DIRECTORIES = alias
        } else {
          const info = join(source.path, '.git', 'objects', 'info')
          await FileSystem.mkdir(info, { recursive: true })
          if (route === 'transitive') {
            const intermediateInfo = join(
              intermediate.path,
              '.git',
              'objects',
              'info'
            )
            await FileSystem.mkdir(intermediateInfo, { recursive: true })
            await FileSystem.writeFile(
              join(intermediateInfo, 'alternates'),
              `${alias}\n`
            )
          }
          await FileSystem.writeFile(
            join(info, 'alternates'),
            `${
              route === 'transitive'
                ? join(intermediate.path, '.git', 'objects')
                : alias
            }\n`
          )
        }
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
        const h = await harness.createAssistedCommitRunHarness(t)
        const repository = await h.register(source)
        h.dispatcher.setPushAfterAssistedCommit(repository, true)
        afterPush = async () => {
          afterPush = undefined
          await FileSystem.unlink(alias)
          await FileSystem.symlink(
            join(foreign.path, '.git', 'objects'),
            alias,
            'junction'
          )
        }
        capturePushes = true
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'pushed')
        assert.strictEqual(
          await fixtures.rawGit(source, [
            'for-each-ref',
            '--format=%(objectname)',
            'refs/remotes/origin/borrowed',
          ]),
          ''
        )
        assert.strictEqual(releasedForTimeout, false)
        assert.ok(outcome.refreshError)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(pushArguments.length, 1)
      } finally {
        clearTimeout(timer)
        lease.release()
        if (previous === undefined) {
          delete process.env.GIT_ALTERNATE_OBJECT_DIRECTORIES
        } else {
          process.env.GIT_ALTERNATE_OBJECT_DIRECTORIES = previous
        }
      }
    })
  }

  it('refuses an alternate pack route changed toward an unadmitted leased store after acknowledgement', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const originalAlternate = await fixtures.seed(t, {
      original: 'Original alternate\n',
    })
    const foreign = await fixtures.seed(t, {
      foreign: 'Packed foreign object\n',
    })
    await fixtures.rawGit(foreign, ['repack', '-a', '-d'])
    await fixtures.rawGit(foreign, [
      'remote',
      'add',
      '--',
      'receiver',
      remote.path,
    ])
    await fixtures.rawGit(foreign, [
      'push',
      '--',
      'receiver',
      'HEAD:refs/heads/borrowed',
    ])
    const info = join(source.path, '.git', 'objects', 'info')
    const pack = join(originalAlternate.path, '.git', 'objects', 'pack')
    await FileSystem.mkdir(info, { recursive: true })
    await FileSystem.mkdir(pack, { recursive: true })
    await FileSystem.writeFile(
      join(info, 'alternates'),
      `${join(originalAlternate.path, '.git', 'objects')}\n`
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let lease:
      | Awaited<ReturnType<typeof operations.acquireAssistedCommitGitLease>>
      | undefined
    let timer: NodeJS.Timeout | undefined
    let releasedForTimeout = false
    afterPush = async () => {
      afterPush = undefined
      lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      await FileSystem.rename(pack, `${pack}-original`)
      await FileSystem.symlink(
        join(foreign.path, '.git', 'objects', 'pack'),
        pack,
        'junction'
      )
      timer = setTimeout(() => {
        releasedForTimeout = true
        lease?.release()
      }, 5000)
    }
    capturePushes = true
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(releasedForTimeout, false)
      assert.strictEqual(
        await fixtures.rawGit(source, [
          'for-each-ref',
          '--format=%(objectname)',
          'refs/remotes/origin/borrowed',
        ]),
        ''
      )
      assert.ok(outcome.refreshError)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
    } finally {
      clearTimeout(timer)
      lease?.release()
    }
  })

  for (const empty of [false, true]) {
    it(`uses a native external object root with no unused default directory for ${
      empty ? 'Empty' : 'selected'
    } commits`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const defaultObjects = join(source.path, '.git', 'objects')
      const objects = join(
        await temporary.createTempDirectory(t),
        'external-objects'
      )
      await FileSystem.rename(defaultObjects, objects)
      const previous = process.env.GIT_OBJECT_DIRECTORY
      process.env.GIT_OBJECT_DIRECTORY = objects
      try {
        if (!empty) {
          await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
        }
        const h = await harness.createAssistedCommitRunHarness(t)
        const repository = await h.register(source)
        h.dispatcher.setPushAfterAssistedCommit(repository, true)
        h.dispatcher.updateCommitOptions(repository, {
          allowEmptyCommit: empty,
        })
        capturePushes = true
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'pushed')
        assert.strictEqual(outcome.refreshError, undefined)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.ok(outcome.result.head.sha !== null)
        assert.strictEqual(
          await fixtures.rawGit(
            source,
            [
              '--git-dir',
              remote.path,
              'cat-file',
              '-t',
              outcome.result.head.sha,
            ],
            { env: { GIT_OBJECT_DIRECTORY: undefined } }
          ),
          'commit'
        )
        assert.strictEqual(h.propose.mock.callCount(), empty ? 0 : 1)
        assert.strictEqual(pushArguments.length, 1)
        await assert.rejects(FileSystem.lstat(defaultObjects), {
          code: 'ENOENT',
        })
        if (empty) {
          assert.strictEqual(
            await fixtures.rawGit(source, [
              'show',
              '-s',
              '--format=%s',
              'HEAD',
            ]),
            'Empty commit'
          )
        }
      } finally {
        if (previous === undefined) {
          delete process.env.GIT_OBJECT_DIRECTORY
        } else {
          process.env.GIT_OBJECT_DIRECTORY = previous
        }
      }
    })
  }

  it('never prunes through a substituted packed-refs target after acknowledged push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const foreignPath = join(
      await temporary.createTempDirectory(t),
      'foreign-packed'
    )
    await fixtures.rawGit(source, ['clone', '--', source.path, foreignPath])
    const foreign = new repositories.Repository(foreignPath, -1, null, false)
    await fixtures.rawGit(foreign, [
      'update-ref',
      'refs/remotes/origin/obsolete',
      remote.originalTip,
    ])
    await fixtures.rawGit(foreign, ['pack-refs', '--all', '--prune'])
    await fixtures.rawGit(source, ['pack-refs', '--all', '--prune'])
    const target = join(foreign.path, '.git', 'packed-refs')
    const original = await FileSystem.readFile(target)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    afterPush = async () => {
      afterPush = undefined
      const route = join(source.path, '.git', 'packed-refs')
      await FileSystem.rename(route, `${route}-original`)
      await FileSystem.symlink(target, route, 'file')
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.deepStrictEqual(await FileSystem.readFile(target), original)
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.pushed, true)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const route of ['primary', 'transitive'] as const) {
    it(`refuses a changed ${route} alternate graph before reading an unadmitted leased object store`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const originalAlternate = await fixtures.seed(t, {
        original: 'Original alternate\n',
      })
      const foreign = await fixtures.seed(t, { foreign: 'Unadmitted object\n' })
      await fixtures.rawGit(foreign, [
        'remote',
        'add',
        '--',
        'receiver',
        remote.path,
      ])
      await fixtures.rawGit(foreign, [
        'push',
        '--',
        'receiver',
        'HEAD:refs/heads/borrowed',
      ])
      const primaryInfo = join(source.path, '.git', 'objects', 'info')
      const alternateInfo = join(
        originalAlternate.path,
        '.git',
        'objects',
        'info'
      )
      await FileSystem.mkdir(primaryInfo, { recursive: true })
      await FileSystem.mkdir(alternateInfo, { recursive: true })
      if (route === 'transitive') {
        await FileSystem.writeFile(
          join(primaryInfo, 'alternates'),
          `${join(originalAlternate.path, '.git', 'objects')}\n`
        )
      }
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      let lease:
        | Awaited<ReturnType<typeof operations.acquireAssistedCommitGitLease>>
        | undefined
      let timer: NodeJS.Timeout | undefined
      let releasedForTimeout = false
      afterPush = async () => {
        afterPush = undefined
        lease = await operations.acquireAssistedCommitGitLease(foreign.path)
        await FileSystem.writeFile(
          join(route === 'primary' ? primaryInfo : alternateInfo, 'alternates'),
          `${join(foreign.path, '.git', 'objects')}\n`
        )
        timer = setTimeout(() => {
          releasedForTimeout = true
          lease?.release()
        }, 5000)
      }
      capturePushes = true
      try {
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'pushed')
        assert.strictEqual(releasedForTimeout, false)
        assert.strictEqual(
          await fixtures.rawGit(source, [
            'for-each-ref',
            '--format=%(objectname)',
            'refs/remotes/origin/borrowed',
          ]),
          ''
        )
        assert.ok(outcome.refreshError)
        const state = h.state(repository).changesState.assistedCommit
        assert.ok(state.kind === 'error')
        assert.strictEqual(state.pushed, true)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(pushArguments.length, 1)
        assert.strictEqual(h.propose.mock.callCount(), 1)
      } finally {
        clearTimeout(timer)
        lease?.release()
      }
      assert.strictEqual(await fixtures.count(foreign), 1)
    })
  }

  for (const routing of [
    'loose',
    'packed',
    'replace-base',
    'namespace',
  ] as const) {
    it(`refuses introduced ${routing} replacement history after acknowledgement`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const gitStore = h.appStore['gitStoreCache'].get(repository)
      let installedHistory: ReadonlyArray<string> | null | undefined
      const previousBase = process.env.GIT_REPLACE_REF_BASE
      const previousNamespace = process.env.GIT_NAMESPACE
      let changed = false
      afterPush = async () => {
        afterPush = undefined
        const sha = await fixtures.tip(source)
        const tree = await fixtures.rawGit(source, [
          'rev-parse',
          `${sha}^{tree}`,
        ])
        const replacement = await fixtures.rawGit(source, [
          'commit-tree',
          tree,
          '-m',
          'External parentless replacement',
        ])
        if (routing === 'replace-base') {
          process.env.GIT_REPLACE_REF_BASE = 'refs/external-replacements/'
        }
        if (routing === 'namespace') {
          process.env.GIT_NAMESPACE = 'external'
        }
        const prefix = process.env.GIT_REPLACE_REF_BASE ?? 'refs/replace/'
        await fixtures.rawGit(source, [
          'update-ref',
          `${prefix}${sha}`,
          replacement,
        ])
        if (routing === 'packed') {
          await fixtures.rawGit(source, ['pack-refs', '--all'])
        }
        changed = true
      }
      afterFollowUpFetch = async () => {
        afterFollowUpFetch = undefined
        installedHistory = await gitStore.loadCommitBatch('HEAD', 0)
      }
      capturePushes = true
      try {
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'pushed')
        assert.strictEqual(changed, true)
        assert.ok(
          installedHistory === undefined ||
            installedHistory?.includes(remote.originalTip),
          `Replacement history installed ${JSON.stringify(installedHistory)}`
        )
        assert.ok(outcome.refreshError)
        const state = h.state(repository).changesState.assistedCommit
        assert.ok(state.kind === 'error')
        assert.strictEqual(state.pushed, true)
        assert.strictEqual(state.retry, null)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
        assert.strictEqual(
          await fixtures.rawGit(source, [
            '--no-replace-objects',
            'rev-list',
            '--count',
            'HEAD',
            '--',
          ]),
          '2'
        )
        assert.strictEqual(pushArguments.length, 1)
        assert.strictEqual(h.propose.mock.callCount(), 1)
        assert.strictEqual(
          await FileSystem.readFile(join(source.path, 'file'), 'utf8'),
          'after\n'
        )
      } finally {
        if (previousBase === undefined) {
          delete process.env.GIT_REPLACE_REF_BASE
        } else {
          process.env.GIT_REPLACE_REF_BASE = previousBase
        }
        if (previousNamespace === undefined) {
          delete process.env.GIT_NAMESPACE
        } else {
          process.env.GIT_NAMESPACE = previousNamespace
        }
      }
    })
  }

  for (const metadata of ['shallow', 'grafts'] as const) {
    for (const route of ['native', 'environment'] as const) {
      for (const replacement of ['symlink', 'hardlink', 'regular'] as const) {
        it(`certifies ${route} ${metadata} metadata before post-ACK History, replacement ${replacement}`, async t => {
          reset()
          t.after(reset)
          nodeBackend(t)
          const source = await fixtures.seed(t, { file: 'before\n' })
          const remote = await remotes.addBareRemote(t, source)
          const foreign = await fixtures.seed(t, {
            foreign: 'Foreign history\n',
          })
          await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
          const h = await harness.createAssistedCommitRunHarness(t)
          const repository = await h.register(source)
          h.dispatcher.setPushAfterAssistedCommit(repository, true)
          const gitStore = h.appStore['gitStoreCache'].get(repository)
          let installedHistory: ReadonlyArray<string> | null | undefined
          const metadataPath =
            route === 'native'
              ? await fixtures.rawGit(source, [
                  'rev-parse',
                  '--path-format=absolute',
                  '--git-path',
                  metadata === 'grafts' ? 'info/grafts' : metadata,
                ])
              : join(source.path, '.git', `custom-${metadata}`)
          const foreignMetadata = join(foreign.path, '.git', metadata)
          const environmentName =
            metadata === 'shallow' ? 'GIT_SHALLOW_FILE' : 'GIT_GRAFT_FILE'
          const previous = process.env[environmentName]
          if (route === 'environment') {
            if (metadata === 'shallow') {
              await FileSystem.writeFile(metadataPath, '')
            }
            process.env[environmentName] = metadataPath
          }
          let lease:
            | Awaited<
                ReturnType<typeof operations.acquireAssistedCommitGitLease>
              >
            | undefined
          let timer: NodeJS.Timeout | undefined
          let releasedForTimeout = false
          let changed = false
          afterPush = async () => {
            afterPush = undefined
            const sha = await fixtures.tip(source)
            await FileSystem.writeFile(foreignMetadata, `${sha}\n`)
            lease = await operations.acquireAssistedCommitGitLease(foreign.path)
            timer = setTimeout(() => {
              releasedForTimeout = true
              lease?.release()
            }, 60000)
            if (route === 'environment' && metadata === 'shallow') {
              await FileSystem.unlink(metadataPath)
            }
            if (replacement === 'regular') {
              const staged = `${metadataPath}-replacement`
              await FileSystem.writeFile(
                staged,
                metadata === 'shallow' ? '' : `${sha}\n`
              )
              await FileSystem.rename(staged, metadataPath)
            } else if (replacement === 'hardlink') {
              await FileSystem.link(foreignMetadata, metadataPath)
            } else {
              await FileSystem.symlink(foreignMetadata, metadataPath, 'file')
            }
            changed = true
          }
          afterFollowUpFetch = async () => {
            afterFollowUpFetch = undefined
            installedHistory = await gitStore.loadCommitBatch('HEAD', 0)
          }
          capturePushes = true
          try {
            const outcome = await h.dispatcher.createCopilotAssistedCommits(
              repository,
              h.request(repository)
            )
            assert.ok(outcome.kind === 'pushed')
            assert.strictEqual(changed, true)
            assert.ok(
              installedHistory === undefined ||
                installedHistory?.includes(remote.originalTip),
              `Foreign ${metadata} metadata installed History ${JSON.stringify(
                installedHistory
              )}`
            )
            const installed = gitStore.commitLookup.get(
              outcome.result.head.sha ?? ''
            )
            assert.ok(
              installed === undefined ||
                installed.parentSHAs.includes(remote.originalTip),
              `Foreign ${metadata} metadata installed parents ${JSON.stringify(
                installed?.parentSHAs
              )}`
            )
            if (replacement === 'regular' && metadata === 'shallow') {
              assert.strictEqual(outcome.refreshError, undefined)
            } else {
              assert.ok(outcome.refreshError)
              const state = h.state(repository).changesState.assistedCommit
              assert.ok(state.kind === 'error')
              assert.strictEqual(state.pushed, true)
              assert.strictEqual(state.retry, null)
            }
            assert.strictEqual(releasedForTimeout, false)
            assert.strictEqual(await remote.tip(), outcome.result.head.sha)
            assert.strictEqual(
              await fixtures.tip(source),
              outcome.result.head.sha
            )
            assert.strictEqual(
              await fixtures
                .rawGit(source, [
                  'cat-file',
                  '-p',
                  outcome.result.head.sha ?? '',
                ])
                .then(value =>
                  value.includes(`parent ${remote.originalTip}\n`)
                ),
              true
            )
            assert.strictEqual(pushArguments.length, 1)
            assert.strictEqual(h.propose.mock.callCount(), 1)
            assert.strictEqual(
              await FileSystem.readFile(join(source.path, 'file'), 'utf8'),
              'after\n'
            )
          } finally {
            clearTimeout(timer)
            lease?.release()
            if (previous === undefined) {
              delete process.env[environmentName]
            } else {
              process.env[environmentName] = previous
            }
          }
        })
      }
    }
  }

  for (const failureCode of ['ENOTDIR', 'ELOOP'] as const) {
    for (const empty of [false, true]) {
      it(`keeps valid local ${
        empty ? 'Empty' : 'selected'
      } commits when an inactive configuration path has ${failureCode}`, async t => {
        reset()
        t.after(reset)
        const source = await fixtures.seed(t, { file: 'before\n' })
        const remote = await remotes.addBareRemote(t, source)
        const parent = join(
          await temporary.createTempDirectory(t),
          'inactive-layout'
        )
        const target = join(parent, 'ignored.conf')
        if (failureCode === 'ENOTDIR') {
          await FileSystem.writeFile(parent, 'Regular-file ancestor\n')
        } else {
          await FileSystem.mkdir(parent)
        }
        await fixtures.rawGit(source, [
          'config',
          '--local',
          '--',
          'includeIf.onbranch:another-layer5-branch.path',
          target,
        ])
        if (!empty) {
          await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
        }
        const h = await harness.createAssistedCommitRunHarness(t)
        const repository = await h.register(source)
        await h.dispatcher.setCommitMessage(repository, {
          summary: 'Preserved manual draft',
          description: 'Preserved manual description',
          timestamp: 1,
        })
        h.dispatcher.setPushAfterAssistedCommit(repository, true)
        h.dispatcher.updateCommitOptions(repository, {
          allowEmptyCommit: empty,
        })
        const failure = Object.assign(
          new Error('Synthetic inactive configuration symlink loop'),
          { code: 'ELOOP', syscall: 'lstat' }
        )
        if (failureCode === 'ELOOP') {
          configurationReadFailure = { path: target, error: failure }
        }
        capturePushes = true
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(
          outcome.kind === 'push-error',
          outcome.kind === 'error'
            ? errors
                .assistedCommitErrorCauses(outcome.error)
                .map(cause =>
                  cause instanceof Error ? cause.stack : String(cause)
                )
                .join('\n')
            : outcome.kind
        )
        assert.strictEqual(outcome.attempted, false)
        assert.ok(
          errors
            .assistedCommitErrorCauses(outcome.error)
            .some(
              cause =>
                cause instanceof Error &&
                'code' in cause &&
                (cause.code === failureCode ||
                  (__WIN32__ &&
                    failureCode === 'ENOTDIR' &&
                    cause.code === 'ENOENT'))
            )
        )
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(await remote.tip(), remote.originalTip)
        assert.strictEqual(pushArguments.length, 0)
        assert.strictEqual(h.propose.mock.callCount(), empty ? 0 : 1)
        if (empty) {
          assert.strictEqual(
            await fixtures.rawGit(source, [
              'show',
              '-s',
              '--format=%s',
              'HEAD',
            ]),
            'Empty commit'
          )
        }
        assert.strictEqual(
          h.state(repository).changesState.commitMessage.summary,
          'Preserved manual draft'
        )
        assert.strictEqual(
          h.state(repository).changesState.commitMessage.description,
          'Preserved manual description'
        )
      })
    }
  }

  for (const cleanupFault of [false, true]) {
    it(`expires a non-directory configuration ancestor only after owned cleanup, fault ${cleanupFault}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      const parent = join(
        await temporary.createTempDirectory(t),
        'inactive-layout'
      )
      const target = join(parent, 'ignored.conf')
      await FileSystem.mkdir(parent)
      await FileSystem.writeFile(target, '')
      await fixtures.rawGit(source, [
        'config',
        '--local',
        '--',
        'includeIf.onbranch:another-layer5-branch.path',
        target,
      ])
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const lock = join(source.path, '.git', 'config.lock')
      duringConfigWrite = async () => {
        duringConfigWrite = undefined
        await FileSystem.rename(parent, `${parent}-original`)
        await FileSystem.writeFile(parent, 'Replacement regular ancestor\n')
      }
      if (cleanupFault) {
        denyUnlink = await fixtures.createPathMatcher(lock)
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      let state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, cleanupFault ? 'refresh' : null)
      if (cleanupFault) {
        assert.strictEqual((await FileSystem.lstat(lock)).isFile(), true)
        denyUnlink = undefined
        await h.dispatcher.retryCopilotAssistedCommitRecovery(
          repository,
          state.runId
        )
        state = h.state(repository).changesState.assistedCommit
        assert.ok(state.kind === 'error')
        assert.strictEqual(state.retry, null)
      }
      await assert.rejects(FileSystem.lstat(lock), { code: 'ENOENT' })
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    })
  }

  it('fences an explicit GIT_DIR alias independently of linked commondir and configuration paths', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const foreign = await fixtures.seed(t, { unrelated: 'Foreign checkout\n' })
    await fixtures.rawGit(foreign, [
      'remote',
      'add',
      '--',
      'origin',
      remote.path,
    ])
    const linked = join(await temporary.createTempDirectory(t), 'linked')
    await fixtures.rawGit(source, [
      'worktree',
      'add',
      '-b',
      'linked-source',
      '--',
      linked,
      'HEAD',
    ])
    const linkedRepository = new repositories.Repository(
      linked,
      source.id,
      null,
      false
    )
    const directory = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--absolute-git-dir',
    ])
    const common = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--path-format=absolute',
      '--git-common-dir',
    ])
    await FileSystem.writeFile(join(directory, 'commondir'), `${common}\n`)
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'branch.linked-source.remote',
      'origin',
    ])
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'branch.linked-source.merge',
      'refs/heads/linked-source',
    ])
    await remote.read([
      'update-ref',
      'refs/heads/linked-source',
      remote.originalTip,
    ])
    const route = join(await temporary.createTempDirectory(t), 'git-dir')
    await FileSystem.symlink(directory, route, 'junction')
    const foreignFetchHead = await fixtures.optionalBytes(
      join(foreign.path, '.git', 'FETCH_HEAD')
    )
    const previous = process.env.GIT_DIR
    process.env.GIT_DIR = route
    try {
      await FileSystem.writeFile(join(linked, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(linkedRepository)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      afterPush = async () => {
        afterPush = undefined
        await FileSystem.unlink(route)
        await FileSystem.symlink(join(foreign.path, '.git'), route, 'junction')
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.deepStrictEqual(
        await fixtures.optionalBytes(join(foreign.path, '.git', 'FETCH_HEAD')),
        foreignFetchHead
      )
      assert.ok(outcome.refreshError)
      assert.strictEqual(
        await remote.read([
          'rev-parse',
          '--verify',
          'refs/heads/linked-source',
        ]),
        outcome.result.head.sha
      )
      assert.strictEqual(pushArguments.length, 1)
    } finally {
      if (previous === undefined) {
        delete process.env.GIT_DIR
      } else {
        process.env.GIT_DIR = previous
      }
    }
    assert.strictEqual(await fixtures.count(linkedRepository), 2)
    assert.strictEqual(await fixtures.count(foreign), 1)
  })

  it('cleans original publication locks before expiring an execution alias held by a foreign lease', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const foreign = await fixtures.seed(t, { unrelated: 'Foreign checkout\n' })
    const alias = join(
      await temporary.createTempDirectory(t),
      'execution-alias'
    )
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const branch = remote.branchRef.slice('refs/heads/'.length)
    const lock = join(source.path, '.git', 'config.lock')
    denyConfigKey = `branch.${branch}.merge`
    denyUnlink = await fixtures.createPathMatcher(lock)
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    assert.strictEqual(
      h.assisted.state['runs'].get(repository.id)?.lease,
      undefined
    )
    assert.strictEqual((await FileSystem.lstat(lock)).isFile(), true)
    denyConfigKey = undefined
    denyUnlink = undefined
    await FileSystem.unlink(alias)
    await FileSystem.symlink(foreign.path, alias, 'junction')
    const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
    let timer: NodeJS.Timeout | undefined
    const retry = h.dispatcher.retryCopilotAssistedCommitRecovery(
      new repositories.Repository(source.path, repository.id, null, false),
      state.runId
    )
    try {
      const completed = await Promise.race([
        retry.then(() => true),
        new Promise<boolean>(resolve => {
          timer = setTimeout(() => resolve(false), 1000)
        }),
      ])
      assert.strictEqual(completed, true)
      await assert.rejects(FileSystem.lstat(lock), { code: 'ENOENT' })
      const settled = h.state(repository).changesState.assistedCommit
      assert.ok(settled.kind === 'error')
      assert.strictEqual(settled.retry, null)
      assert.strictEqual(settled.settling, false)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      assert.strictEqual(await fixtures.count(source), 2)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      clearTimeout(timer)
      lease.release()
      await retry
    }
    assert.strictEqual(await fixtures.count(foreign), 1)
  })

  it('never recursively fetches into an unadmitted submodule repository after acknowledged push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const child = await fixtures.seed(t, { child: 'before\n' })
    const childRemote = await remotes.addBareRemote(t, child)
    await childRemote.read(['symbolic-ref', 'HEAD', childRemote.branchRef])
    const source = await fixtures.seed(t, { file: 'before\n' })
    await fixtures.rawGit(source, [
      '-c',
      'protocol.file.allow=always',
      'submodule',
      'add',
      '--',
      childRemote.path,
      'module',
    ])
    await fixtures.rawGit(source, ['commit', '-m', 'Add clean submodule'])
    const remote = await remotes.addBareRemote(t, source)
    const foreignPath = join(
      await temporary.createTempDirectory(t),
      'foreign-child'
    )
    await fixtures.rawGit(child, ['clone', '--', childRemote.path, foreignPath])
    const foreign = new repositories.Repository(foreignPath, -1, null, false)
    await fixtures.rawGit(foreign, [
      'config',
      '--local',
      '--',
      'protocol.file.allow',
      'always',
    ])
    const foreignRef = `refs/remotes/origin/${childRemote.branchRef.slice(
      'refs/heads/'.length
    )}`
    const foreignTip = await fixtures.rawGit(foreign, [
      'rev-parse',
      '--verify',
      foreignRef,
    ])
    const foreignFetchHead = await fixtures.optionalBytes(
      join(foreign.path, '.git', 'FETCH_HEAD')
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let receiverTip: string | undefined
    afterPush = async () => {
      afterPush = undefined
      capturePushes = false
      try {
        await FileSystem.writeFile(
          join(child.path, 'child'),
          'Receiver child\n'
        )
        await fixtures.rawGit(child, ['add', '--', 'child'])
        await fixtures.rawGit(child, ['commit', '-m', 'Receiver child update'])
        await fixtures.rawGit(child, [
          'push',
          '--',
          'origin',
          childRemote.branchRef,
        ])
        const childTip = await fixtures.tip(child)
        const parentTip = await remote.tip()
        const entries = await remote.read(['ls-tree', `${parentTip}^{tree}`])
        const updated = entries.replace(
          /160000 commit [0-9a-f]+\tmodule(?=\n|$)/,
          `160000 commit ${childTip}\tmodule`
        )
        assert.notStrictEqual(updated, entries)
        const tree = await fixtures.rawGit(
          source,
          ['--git-dir', remote.path, 'mktree'],
          { stdin: `${updated}\n` }
        )
        receiverTip = await remote.read([
          'commit-tree',
          tree,
          '-p',
          parentTip,
          '-m',
          'Receiver submodule update',
        ])
        await remote.read([
          'update-ref',
          remote.branchRef,
          receiverTip,
          parentTip,
        ])
        const modulePath = join(source.path, '.git', 'modules', 'module')
        await FileSystem.rename(modulePath, `${modulePath}-original`)
        await FileSystem.symlink(
          join(foreign.path, '.git'),
          modulePath,
          'junction'
        )
      } finally {
        capturePushes = true
      }
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(
      await fixtures.rawGit(foreign, ['rev-parse', '--verify', foreignRef]),
      foreignTip
    )
    assert.deepStrictEqual(
      await fixtures.optionalBytes(join(foreign.path, '.git', 'FETCH_HEAD')),
      foreignFetchHead
    )
    assert.strictEqual(await remote.tip(), receiverTip)
    assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 3)
    assert.strictEqual(await fixtures.count(foreign), 1)
    assert.strictEqual(pushArguments.length, 1)
    assert.strictEqual(h.propose.mock.callCount(), 1)
  })

  for (const variable of ['GIT_CONFIG_GLOBAL', 'GIT_CONFIG_SYSTEM'] as const) {
    it(`preserves literal ${
      __WIN32__ ? 'space-and-apostrophe' : 'newline-bearing'
    } ${variable} paths during certification`, async t => {
      reset()
      t.after(reset)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const target = join(
        await temporary.createTempDirectory(t),
        __WIN32__
          ? "missing configuration's literal path"
          : 'missing\nconfiguration'
      )
      const failure = Object.assign(
        new Error('Literal configuration input EIO'),
        {
          code: 'EIO',
        }
      )
      const previous = process.env[variable]
      const previousNoSystem = process.env.GIT_CONFIG_NOSYSTEM
      process.env[variable] = target
      process.env.GIT_CONFIG_NOSYSTEM = '0'
      configurationReadFailure = { path: target, error: failure }
      try {
        await assert.rejects(
          h.dispatcher.prepareCopilotAssistedCommitRequest(
            repository,
            h.request(repository)
          ),
          error => errors.assistedCommitErrorCauses(error).includes(failure)
        )
        assert.strictEqual(h.propose.mock.callCount(), 0)
        assert.strictEqual(await remote.tip(), remote.originalTip)
        assert.strictEqual(await fixtures.count(source), 1)
      } finally {
        if (previous === undefined) {
          delete process.env[variable]
        } else {
          process.env[variable] = previous
        }
        if (previousNoSystem === undefined) {
          delete process.env.GIT_CONFIG_NOSYSTEM
        } else {
          process.env.GIT_CONFIG_NOSYSTEM = previousNoSystem
        }
      }
    })
  }

  it('refuses follow-up fetch through a substituted object-pack directory without writing foreign packs', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Unrelated checkout\n',
    })
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'fetch.unpackLimit',
      '1',
    ])
    const foreign = join(replacement.path, '.git', 'objects', 'pack')
    const route = join(source.path, '.git', 'objects', 'pack')
    await FileSystem.mkdir(foreign, { recursive: true })
    await FileSystem.mkdir(route, { recursive: true })
    const original = await FileSystem.readdir(foreign)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let replaced = false
    afterPush = async () => {
      afterPush = undefined
      const tree = await remote.read([
        'rev-parse',
        '--verify',
        `${remote.originalTip}^{tree}`,
      ])
      const extra = await remote.read([
        'commit-tree',
        tree,
        '-p',
        remote.originalTip,
        '-m',
        'Receiver-only packed commit',
      ])
      await remote.read(['update-ref', 'refs/heads/unfetched', extra])
      await FileSystem.rename(route, `${route}-original`)
      await FileSystem.symlink(foreign, route, 'junction')
      replaced = true
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(replaced, true)
    assert.deepStrictEqual(await FileSystem.readdir(foreign), original)
    assert.ok(outcome.refreshError)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('refuses retargeted effective GIT_WORK_TREE routing after acknowledged push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    await FileSystem.writeFile(
      join(replacement.path, 'foreign'),
      'Foreign changes\n'
    )
    const route = join(await temporary.createTempDirectory(t), 'worktree')
    await FileSystem.symlink(source.path, route, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const previous = process.env.GIT_WORK_TREE
    process.env.GIT_WORK_TREE = route
    try {
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      afterPush = async () => {
        afterPush = undefined
        await FileSystem.unlink(route)
        await FileSystem.symlink(replacement.path, route, 'junction')
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(
        h
          .state(repository)
          .changesState.workingDirectory.files.some(
            file => file.path === 'foreign'
          ),
        false
      )
      assert.ok(outcome.refreshError)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
    } finally {
      if (previous === undefined) {
        delete process.env.GIT_WORK_TREE
      } else {
        process.env.GIT_WORK_TREE = previous
      }
    }
  })

  it('refuses follow-up fetch through a substituted reflog directory without appending foreign bytes', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'core.logAllRefUpdates',
      'true',
    ])
    const foreign = await temporary.createTempDirectory(t)
    const sentinel = join(foreign, 'unfetched')
    const bytes = Buffer.from('Never append a foreign reflog\n')
    await FileSystem.writeFile(sentinel, bytes)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let replaced = false
    afterPush = async () => {
      afterPush = undefined
      const tree = await remote.read([
        'rev-parse',
        '--verify',
        `${remote.originalTip}^{tree}`,
      ])
      const extra = await remote.read([
        'commit-tree',
        tree,
        '-p',
        remote.originalTip,
        '-m',
        'Receiver-only reflog branch',
      ])
      await remote.read(['update-ref', 'refs/heads/unfetched', extra])
      const path = join(
        source.path,
        '.git',
        'logs',
        'refs',
        'remotes',
        'origin'
      )
      await FileSystem.rename(path, `${path}-original`)
      await FileSystem.symlink(foreign, path, 'junction')
      replaced = true
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(replaced, true)
    assert.deepStrictEqual(await FileSystem.readFile(sentinel), bytes)
    assert.ok(outcome.refreshError)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const route of ['FETCH_HEAD', 'index']) {
    it(`refuses post-ACK ${route} symlink routing without foreign writes or Changes`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const replacement = await fixtures.seed(t, {
        file: 'before\n',
        foreign: 'Foreign index entry\n',
      })
      const sentinel = join(await temporary.createTempDirectory(t), 'sentinel')
      const sentinelBytes = Buffer.from('Never overwrite foreign metadata\n')
      await FileSystem.writeFile(sentinel, sentinelBytes)
      const target =
        route === 'index' ? join(replacement.path, '.git', 'index') : sentinel
      const originalTarget = await FileSystem.readFile(target)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      afterPush = async () => {
        afterPush = undefined
        const path = join(source.path, '.git', route)
        await FileSystem.rm(path, { force: true })
        await FileSystem.symlink(target, path, 'file')
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.deepStrictEqual(await FileSystem.readFile(target), originalTarget)
      assert.strictEqual(
        h
          .state(repository)
          .changesState.workingDirectory.files.some(
            file => file.path === 'foreign'
          ),
        false
      )
      assert.ok(outcome.refreshError)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
    })
  }

  it('rejects an alias changed after canonical lookup but before capture', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    const requested = new repositories.Repository(
      alias,
      repository.id,
      null,
      false
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    statusFailure = new Error('Synthetic first refresh failure')
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    let replaced = false
    aliasResolutionPath = alias
    beforeAliasFenceCapture = async () => {
      beforeAliasFenceCapture = undefined
      await FileSystem.unlink(alias)
      await FileSystem.symlink(replacement.path, alias, 'junction')
      replaced = true
    }
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      requested,
      state.runId
    )
    assert.strictEqual(replaced, true)
    const refused = h.state(repository).changesState.assistedCommit
    assert.ok(refused.kind === 'error')
    assert.strictEqual(refused.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('never installs a foreign untracked image during accepted reader settlement', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    const originalImage = await FileSystem.readFile(
      join(
        'app',
        'test',
        'fixtures',
        'repo-with-image-changes',
        'new-image.png'
      )
    )
    const foreignImage = await FileSystem.readFile(
      join(
        'app',
        'test',
        'fixtures',
        'detect-conflict-in-binary-file',
        'my-cool-image.png'
      )
    )
    assert.ok(!originalImage.equals(foreignImage))
    await FileSystem.writeFile(join(source.path, 'image.png'), originalImage)
    await FileSystem.writeFile(
      join(replacement.path, 'image.png'),
      foreignImage
    )
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    const image = h
      .state(repository)
      .changesState.workingDirectory.files.find(
        file => file.path === 'image.png'
      )
    assert.ok(image !== undefined)
    await h.dispatcher.changeFileIncluded(repository, image, false)
    await h.dispatcher.selectWorkingDirectoryFiles(repository, [image])
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let retargeted = false
    let readerError: unknown
    let injected = false
    let foreignFrames = 0
    const emit = h.appStore['emitUpdate'].bind(h.appStore)
    h.appStore['emitUpdate'] = () => {
      const selection = h.state(repository).changesState.selection
      if (
        selection.kind ===
          selectionStates.ChangesSelectionKind.WorkingDirectory &&
        selection.diff?.kind === diffs.DiffType.Image &&
        selection.diff.current?.contents === foreignImage.toString('base64')
      ) {
        foreignFrames++
      }
      emit()
    }
    const finish = h.assisted.readers['finish'].bind(h.assisted.readers)
    h.assisted.readers['finish'] = async run => {
      if (run.pushSucceeded && !injected) {
        injected = true
        beforeImageRead = async () => {
          beforeImageRead = undefined
          await FileSystem.unlink(alias)
          await FileSystem.symlink(replacement.path, alias, 'junction')
          retargeted = true
        }
        try {
          await h.appStore['updateChangesWorkingDirectoryDiff'](repository)
        } catch (error) {
          readerError = error
        }
      }
      return finish(run)
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(retargeted, true)
    assert.strictEqual(foreignFrames, 0)
    const selection = h.state(repository).changesState.selection
    assert.ok(
      selection.kind === selectionStates.ChangesSelectionKind.WorkingDirectory
    )
    assert.ok(
      selection.diff?.kind !== diffs.DiffType.Image ||
        selection.diff.current?.contents !== foreignImage.toString('base64')
    )
    assert.ok(outcome.refreshError)
    assert.ok(readerError instanceof Error)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.deepStrictEqual(
      await FileSystem.readFile(join(source.path, 'image.png')),
      originalImage
    )
    assert.strictEqual(pushArguments.length, 1)
  })

  it('never fetches into a substituted object-directory alias after acknowledged push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    const objects = join(source.path, '.git', 'objects')
    const otherObjects = join(replacement.path, '.git', 'objects')
    const pool = join(await temporary.createTempDirectory(t), 'objects')
    await FileSystem.rename(objects, pool)
    await FileSystem.symlink(pool, objects, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let receiverOnly: string | undefined
    let retargeted = false
    afterPush = async () => {
      afterPush = undefined
      await FileSystem.cp(pool, otherObjects, { recursive: true })
      const tree = await remote.read([
        'rev-parse',
        '--verify',
        `${remote.originalTip}^{tree}`,
      ])
      receiverOnly = await remote.read([
        'commit-tree',
        tree,
        '-p',
        remote.originalTip,
        '-m',
        'Receiver-only unfetched commit',
      ])
      await remote.read(['update-ref', 'refs/heads/unfetched', receiverOnly])
      await FileSystem.unlink(objects)
      await FileSystem.symlink(otherObjects, objects, 'junction')
      retargeted = true
    }
    capturePushes = true
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(retargeted, true)
      assert.ok(receiverOnly !== undefined)
      const foreignObject = await actualExec(
        ['cat-file', '-e', receiverOnly],
        replacement.path
      )
      assert.notStrictEqual(foreignObject.exitCode, 0)
      assert.ok(outcome.refreshError)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(await fixtures.count(replacement), 1)
      assert.strictEqual(
        await FileSystem.readFile(join(replacement.path, 'foreign'), 'utf8'),
        'Foreign checkout\n'
      )
      assert.strictEqual(pushArguments.length, 1)
    } finally {
      if (retargeted) {
        await FileSystem.unlink(objects)
        await FileSystem.symlink(pool, objects, 'junction')
      }
    }
  })

  for (const reader of ['refresh', 'status'] as const) {
    it(`rejects a substituted accepted ${reader} alias before waiting on a foreign lease`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const foreign = await fixtures.seed(t, { foreign: 'Foreign checkout\n' })
      const alias = join(
        await temporary.createTempDirectory(t),
        'execution-alias'
      )
      await FileSystem.symlink(source.path, alias, 'junction')
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(
        new repositories.Repository(alias, source.id, null, false)
      )
      h.appStore['selectedRepository'] = repository
      let renderedRetry: string | null | undefined
      let renderedError: Error | undefined
      const subscription = h.appStore.onDidUpdate(state => {
        const selected = state.selectedState
        if (selected?.type === selectionStates.SelectionType.Repository) {
          const assisted = selected.state.changesState.assistedCommit
          if (assisted.kind === 'error') {
            renderedRetry = assisted.retry
            renderedError = assisted.error
          }
        }
      })
      t.after(() => subscription.dispose())
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const failure = new Error('Synthetic acknowledged refresh failure')
      afterPush = async () => {
        afterPush = undefined
        statusFailure = failure
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, 'refresh')
      assert.strictEqual(renderedRetry, 'refresh')
      await FileSystem.unlink(alias)
      await FileSystem.symlink(foreign.path, alias, 'junction')
      const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      let timer: NodeJS.Timeout | undefined
      const refresh =
        reader === 'refresh'
          ? h.appStore._refreshRepository(repository)
          : h.appStore._loadStatus(repository)
      try {
        const error = await Promise.race([
          refresh.then(
            () => undefined,
            error => error
          ),
          new Promise<Error>(resolve => {
            timer = setTimeout(
              () =>
                resolve(new Error('Accepted refresh waited on foreign lease')),
              2000
            )
          }),
        ])
        assert.ok(error instanceof Error)
        assert.notStrictEqual(
          error.message,
          'Accepted refresh waited on foreign lease'
        )
        assert.ok(
          errors
            .assistedCommitErrorCauses(error)
            .some(
              cause =>
                cause instanceof Error &&
                cause.message.includes('original repository owner changed')
            )
        )
        const settled = h.state(repository).changesState.assistedCommit
        assert.ok(settled.kind === 'error')
        assert.strictEqual(settled.retry, null)
        assert.strictEqual(renderedRetry, null)
        assert.strictEqual(renderedError, settled.error)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.strictEqual(await fixtures.count(source), 2)
        assert.strictEqual(pushArguments.length, 1)
        assert.strictEqual(h.propose.mock.callCount(), 1)
        assert.strictEqual(
          h.assisted.state['selectionReaders'].has(repository.id),
          false
        )
      } finally {
        clearTimeout(timer)
        lease.release()
        await refresh.catch(() => {})
      }
      assert.strictEqual(await fixtures.count(foreign), 1)
    })
  }

  it('rejects canonical checkout substitution during accepted reader registration before foreign admission', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const foreign = await fixtures.seed(t, { foreign: 'Foreign checkout\n' })
    const alias = join(
      await temporary.createTempDirectory(t),
      'execution-alias'
    )
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    h.appStore['selectedRepository'] = repository
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    afterPush = async () => {
      afterPush = undefined
      statusFailure = new Error('Synthetic acknowledged refresh failure')
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
    const parked = `${source.path}-original`
    let moved = false
    const subscription = h.appStore.onDidUpdate(() => {
      if (!moved && h.assisted.state['selectionReaders'].has(repository.id)) {
        FileSystemSync.renameSync(source.path, parked)
        FileSystemSync.symlinkSync(foreign.path, source.path, 'junction')
        moved = true
      }
    })
    let timer: NodeJS.Timeout | undefined
    const reading = h.appStore._loadStatus(repository)
    try {
      const error = await Promise.race([
        reading.then(
          () => undefined,
          error => error
        ),
        new Promise<Error>(resolve => {
          timer = setTimeout(
            () =>
              resolve(new Error('Registered reader waited on foreign lease')),
            2000
          )
        }),
      ])
      assert.strictEqual(moved, true)
      assert.ok(error instanceof Error)
      assert.notStrictEqual(
        error.message,
        'Registered reader waited on foreign lease'
      )
      assert.strictEqual(
        h.assisted.state['selectionReaders'].has(repository.id),
        false
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      assert.strictEqual(h.propose.mock.callCount(), 1)
    } finally {
      clearTimeout(timer)
      subscription.dispose()
      if (moved) {
        await FileSystem.unlink(source.path)
        await FileSystem.rename(parked, source.path)
      }
      lease.release()
      await reading.catch(() => {})
    }
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(await fixtures.count(foreign), 1)
  })

  for (const retry of ['push', 'refresh'] as const) {
    it(`refuses canonical substitution during ${retry} retry admission without waiting on foreign recovery`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const foreign = await fixtures.seed(t, { foreign: 'Foreign checkout\n' })
      const alias = join(
        await temporary.createTempDirectory(t),
        'execution-alias'
      )
      await FileSystem.symlink(source.path, alias, 'junction')
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(
        new repositories.Repository(alias, source.id, null, false)
      )
      h.appStore['selectedRepository'] = repository
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      if (retry === 'push') {
        reportPushFailure = new Error('Synthetic remote response lost')
      } else {
        afterPush = async () => {
          afterPush = undefined
          statusFailure = new Error('Synthetic acknowledged refresh failure')
        }
      }
      capturePushes = true
      const initial = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.strictEqual(
        initial.kind,
        retry === 'push' ? 'push-error' : 'pushed'
      )
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'push-error' || state.kind === 'error')
      const lease = await operations.acquireAssistedCommitGitLease(foreign.path)
      const parked = `${source.path}-original`
      let moved = false
      const subscription = h.appStore.onDidUpdate(() => {
        const current = h.state(repository).changesState.assistedCommit
        if (
          !moved &&
          current.kind === (retry === 'push' ? 'preparing-push' : 'refreshing')
        ) {
          FileSystemSync.renameSync(source.path, parked)
          FileSystemSync.symlinkSync(foreign.path, source.path, 'junction')
          moved = true
        }
      })
      let timer: NodeJS.Timeout | undefined
      const retrying =
        retry === 'push'
          ? h.dispatcher.retryCopilotAssistedCommitPush(repository, state.runId)
          : h.dispatcher.retryCopilotAssistedCommitRecovery(
              repository,
              state.runId
            )
      try {
        const completed = await Promise.race([
          retrying.then(() => true),
          new Promise<boolean>(resolve => {
            timer = setTimeout(() => resolve(false), 2000)
          }),
        ])
        assert.strictEqual(moved, true)
        assert.strictEqual(
          completed,
          true,
          'Retry waited on foreign recovery lease'
        )
        const settled = h.state(repository).changesState.assistedCommit
        assert.ok(settled.kind === 'push-error' || settled.kind === 'error')
        assert.strictEqual(settled.settling, false)
        assert.strictEqual(pushArguments.length, 1)
        assert.strictEqual(h.propose.mock.callCount(), 1)
      } finally {
        clearTimeout(timer)
        subscription.dispose()
        if (moved) {
          await FileSystem.unlink(source.path)
          await FileSystem.rename(parked, source.path)
        }
        lease.release()
        await retrying
      }
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(await fixtures.count(foreign), 1)
      assert.strictEqual(await remote.tip(), await fixtures.tip(source))
    })
  }

  it('never resumes accepted deferred History through a substituted execution alias', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let retargetedAt = 0
    afterPush = async () => {
      afterPush = undefined
      await FileSystem.cp(
        join(source.path, '.git', 'objects'),
        join(replacement.path, '.git', 'objects'),
        { recursive: true }
      )
      const accepted = await fixtures.tip(source)
      h.dispatcher.changeCommitSelection(repository, [accepted], true)
      h.assisted.readers['deferredHistory'].set(repository.id, repository)
      await FileSystem.unlink(alias)
      await FileSystem.symlink(replacement.path, alias, 'junction')
      retargetedAt = allExecutions.length
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    assert.ok(retargetedAt > 0)
    assert.strictEqual(
      allExecutions
        .slice(retargetedAt)
        .some(
          call =>
            call.path === alias &&
            (call.args.includes('log') || call.args.includes('diff'))
        ),
      false
    )
    assert.strictEqual(
      h.assisted.readers['deferredHistory'].has(repository.id),
      false
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(await fixtures.count(replacement), 1)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const phase of ['deferred refresh', 'selection readers'] as const) {
    it(`returns acknowledged push with actual ${phase} settlement error`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const failure = Object.assign(new Error(`Synthetic final ${phase} EIO`), {
        code: 'EIO',
      })
      let injected = false
      if (phase === 'deferred refresh') {
        const update = h.assisted.state['set'].bind(h.assisted.state)
        h.assisted.state['set'] = (run, state) => {
          update(run, state)
          if (run.pushSucceeded && state.kind === 'idle' && !injected) {
            injected = true
            h.assisted.readers['deferredRefreshes'].set(
              repository.id,
              repository
            )
            statusFailure = failure
          }
        }
      } else {
        const finish = h.assisted.readers['finish'].bind(h.assisted.readers)
        h.assisted.readers['finish'] = async run => {
          await finish(run)
          if (run.pushSucceeded && !injected) {
            injected = true
            h.assisted.readers['invalidate'](repository, run, failure)
          }
        }
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(injected, true)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, 'refresh')
      assert.ok(outcome.refreshError)
      assert.ok(
        errors.assistedCommitErrorCauses(outcome.refreshError).includes(failure)
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
      await h.dispatcher.retryCopilotAssistedCommitRecovery(
        repository,
        state.runId
      )
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
    })
  }

  for (const retry of [false, true]) {
    it(`returns actual final deferred History error after acknowledged push, retry ${retry}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      const failed = retry
        ? await (async () => {
            reportPushFailure = new Error('Synthetic initial response lost')
            return h.dispatcher.createCopilotAssistedCommits(
              repository,
              h.request(repository)
            )
          })()
        : undefined
      if (retry) {
        assert.ok(failed?.kind === 'push-error')
      }
      const failure = Object.assign(
        new Error('Synthetic final deferred History EIO'),
        { code: 'EIO' }
      )
      let queued = false
      const update = h.assisted.state['set'].bind(h.assisted.state)
      h.assisted.state['set'] = (run, state) => {
        update(run, state)
        if (run.pushSucceeded && state.kind === 'idle' && !queued) {
          queued = true
          const sha = run.result?.head.sha
          assert.ok(sha !== undefined && sha !== null)
          h.dispatcher.changeCommitSelection(repository, [sha], true)
          h.assisted.readers['deferredHistory'].set(repository.id, repository)
          historyFailure = failure
        }
      }
      const state = h.state(repository).changesState.assistedCommit
      const outcome = retry
        ? await (async () => {
            assert.ok(state.kind === 'push-error')
            return h.dispatcher.retryCopilotAssistedCommitPush(
              repository,
              state.runId
            )
          })()
        : await h.dispatcher.createCopilotAssistedCommits(
            repository,
            h.request(repository)
          )
      assert.ok(outcome?.kind === 'pushed')
      assert.strictEqual(queued, true)
      assert.ok(outcome.refreshError)
      assert.ok(
        errors.assistedCommitErrorCauses(outcome.refreshError).includes(failure)
      )
      const error = h.state(repository).changesState.assistedCommit
      assert.ok(error.kind === 'error')
      assert.strictEqual(error.retry, 'refresh')
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, retry ? 2 : 1)
      assert.strictEqual(h.propose.mock.callCount(), 1)
      await h.dispatcher.retryCopilotAssistedCommitRecovery(
        repository,
        error.runId
      )
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, retry ? 2 : 1)
    })
  }

  it('checks accepted History ownership before installing an awaited changeset result', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout\n',
    })
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let before = h.state(repository).commitSelection.changesetData
    let retargeted = false
    let queued = false
    const update = h.assisted.state['set'].bind(h.assisted.state)
    h.assisted.state['set'] = (run, state) => {
      update(run, state)
      if (run.pushSucceeded && state.kind === 'idle' && !queued) {
        queued = true
        const sha = run.result?.head.sha
        assert.ok(sha !== undefined && sha !== null)
        h.dispatcher.changeCommitSelection(repository, [sha], true)
        before = h.state(repository).commitSelection.changesetData
        h.assisted.readers['deferredHistory'].set(repository.id, repository)
        afterHistoryRead = async () => {
          afterHistoryRead = undefined
          await FileSystem.unlink(alias)
          await FileSystem.symlink(replacement.path, alias, 'junction')
          retargeted = true
        }
      }
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(queued, true)
    assert.strictEqual(retargeted, true)
    assert.ok(outcome.refreshError)
    assert.strictEqual(
      h.state(repository).commitSelection.changesetData,
      before
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(await fixtures.count(replacement), 1)
    assert.strictEqual(pushArguments.length, 1)
  })

  for (const replacement of [false, true]) {
    it(`refuses changed publication lock contents after awaited close, replacement ${replacement}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      const configPath = join(source.path, '.git', 'config')
      const originalConfig = await FileSystem.readFile(configPath)
      const foreign = Buffer.from(
        '[unrelated]\n\tvalue = "Never install this"\n'
      )
      let substituted = false
      let closedPath: string | undefined
      afterPublicationLockClose = async path => {
        afterPublicationLockClose = undefined
        if (replacement) {
          await FileSystem.rename(path, `${path}-original`)
        }
        await FileSystem.writeFile(path, foreign)
        closedPath = path
        substituted = true
      }
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(substituted, true)
      assert.ok(outcome.refreshError)
      assert.deepStrictEqual(
        await FileSystem.readFile(configPath),
        originalConfig
      )
      if (replacement) {
        assert.ok(closedPath !== undefined)
        assert.deepStrictEqual(await FileSystem.readFile(closedPath), foreign)
      }
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(repository), 2)
      assert.strictEqual(pushArguments.length, 1)
    })
  }

  it('never refreshes a changed deferred alias while settling push-only retry', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, { foreign: 'before\n' })
    await FileSystem.writeFile(
      join(replacement.path, 'foreign'),
      'Foreign edits\n'
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    const requested = new repositories.Repository(
      alias,
      repository.id,
      null,
      false
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    reportPushFailure = new Error('Synthetic first push response lost')
    const failed = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(failed.kind === 'push-error')
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'push-error')
    afterPush = async () => {
      afterPush = undefined
      h.assisted.readers['deferredRefreshes'].set(repository.id, requested)
      await FileSystem.unlink(alias)
      await FileSystem.symlink(replacement.path, alias, 'junction')
    }
    const outcome = await h.dispatcher.retryCopilotAssistedCommitPush(
      requested,
      state.runId
    )
    assert.ok(outcome?.kind === 'pushed')
    assert.ok(outcome.refreshError)
    assert.strictEqual(
      allExecutions.some(
        call => call.path === requested.path && call.args.includes('status')
      ),
      false
    )
    assert.strictEqual(
      h
        .state(repository)
        .changesState.workingDirectory.files.some(
          file => file.path === 'foreign'
        ),
      false
    )
    const refused = h.state(repository).changesState.assistedCommit
    assert.ok(refused.kind === 'error')
    assert.strictEqual(refused.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(h.propose.mock.callCount(), 1)
    assert.strictEqual(pushArguments.length, 2)
  })

  for (const boundary of ['creation', 'marker-release'] as const) {
    it(`never adopts or deletes unrelated publication files at staging ${boundary}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      let foreignDirectory = join(source.path, '.git', 'unrelated-publication')
      await FileSystem.mkdir(foreignDirectory)
      await FileSystem.writeFile(
        join(foreignDirectory, 'sentinel'),
        'Never delete unrelated publication files\n'
      )
      const substitute = async (directory: string) => {
        afterStagingDirectoryCreation = undefined
        afterMarkerUnlink = undefined
        await FileSystem.rename(directory, `${directory}-original`)
        await FileSystem.rename(foreignDirectory, directory)
        foreignDirectory = directory
      }
      if (boundary === 'creation') {
        afterStagingDirectoryCreation = substitute
      } else {
        afterMarkerUnlink = path => substitute(join(path, '..'))
      }
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(outcome.refreshError, undefined)
      assert.strictEqual(stagingDirectoryCreations, 0)
      assert.strictEqual(
        await FileSystem.readFile(join(foreignDirectory, 'sentinel'), 'utf8'),
        'Never delete unrelated publication files\n'
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(repository), 2)
      assert.strictEqual(pushArguments.length, 1)
    })
  }

  it('cannot strand publication recovery through a failed staging marker open', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    markerOpenFailure = Object.assign(
      new Error('Synthetic first staging marker open EIO'),
      { code: 'EIO' }
    )
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    const state = h.state(repository).changesState.assistedCommit
    if (state.kind === 'error') {
      assert.strictEqual(state.retry, 'refresh')
      await h.dispatcher.retryCopilotAssistedCommitRecovery(
        repository,
        state.runId
      )
    }
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
    assert.strictEqual(stagingDirectoryCreations, 0)
    assert.ok(markerOpenFailure !== undefined)
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--local',
        '--get',
        'branch.master.remote',
      ]),
      'origin'
    )
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--local',
        '--get',
        'branch.master.merge',
      ]),
      remote.remoteRef
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(repository), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('rejects a changed requested recovery alias without reading another checkout', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, { foreign: 'before\n' })
    await FileSystem.writeFile(
      join(replacement.path, 'foreign'),
      'Foreign edits\n'
    )
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    const requested = new repositories.Repository(
      alias,
      repository.id,
      null,
      false
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    statusFailure = new Error(
      'Synthetic post-ACK status failure before alias retry'
    )
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    await FileSystem.unlink(alias)
    await FileSystem.symlink(replacement.path, alias, 'junction')
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      requested,
      state.runId
    )
    const refused = h.state(repository).changesState.assistedCommit
    assert.ok(refused.kind === 'error')
    assert.strictEqual(
      allExecutions.some(
        call => call.path === requested.path && call.args.includes('status')
      ),
      false
    )
    assert.strictEqual(
      h
        .state(repository)
        .changesState.workingDirectory.files.some(
          file => file.path === 'foreign'
        ),
      false
    )
    assert.strictEqual(
      refused.retry,
      null,
      errors
        .assistedCommitErrorCauses(refused.error)
        .map(cause =>
          cause instanceof Error
            ? (cause.stack ?? cause.message)
            : String(cause)
        )
        .join('\n')
    )
    assert.strictEqual(
      await FileSystem.readFile(join(replacement.path, 'foreign'), 'utf8'),
      'Foreign edits\n'
    )
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('refuses configuration-backed worktree redirection after acknowledged push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, { foreign: 'before\n' })
    await FileSystem.writeFile(
      join(replacement.path, 'foreign'),
      'Foreign edits\n'
    )
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    afterPush = async path => {
      afterPush = undefined
      const result = await actualExec(
        ['config', '--local', '--', 'core.worktree', replacement.path],
        path
      )
      assert.strictEqual(result.exitCode, 0, result.stderr)
    }
    capturePushes = true
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      assert.strictEqual(
        h
          .state(repository)
          .changesState.workingDirectory.files.some(
            file => file.path === 'foreign'
          ),
        false
      )
      assert.strictEqual(
        await FileSystem.readFile(join(replacement.path, 'foreign'), 'utf8'),
        'Foreign edits\n'
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(pushArguments.length, 1)
    } finally {
      const result = await actualExec(
        ['config', '--local', '--unset-all', '--', 'core.worktree'],
        source.path
      )
      assert.strictEqual(result.exitCode, 0, result.stderr)
    }
  })

  it('never installs another checkout into assisted state when Retry refresh owner changed', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    const replacement = await fixtures.seed(t, {
      foreign: 'Foreign checkout stays\n',
    })
    const alias = join(await temporary.createTempDirectory(t), 'alias')
    await FileSystem.symlink(source.path, alias, 'junction')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(
      new repositories.Repository(alias, source.id, null, false)
    )
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    statusFailure = new Error('Synthetic post-ACK status failure')
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    await FileSystem.rename(alias, `${alias}-original`)
    await FileSystem.symlink(replacement.path, alias, 'junction')
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    const refused = h.state(repository).changesState.assistedCommit
    assert.ok(refused.kind === 'error')
    assert.strictEqual(refused.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(source), 2)
    assert.strictEqual(
      await FileSystem.readFile(join(replacement.path, 'foreign'), 'utf8'),
      'Foreign checkout stays\n'
    )
    assert.strictEqual(pushArguments.length, 1)
    assert.strictEqual(
      h
        .state(repository)
        .changesState.workingDirectory.files.some(
          file => file.path === 'foreign'
        ),
      false
    )
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
  })

  it('never installs tracking into a configuration target retired after path certification', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const alias = join(source.path, '.git', 'config')
    const bytes = await FileSystem.readFile(alias)
    const first = join(await temporary.createTempDirectory(t), 'config')
    const second = join(await temporary.createTempDirectory(t), 'config')
    await FileSystem.writeFile(first, bytes)
    await FileSystem.writeFile(second, bytes)
    await FileSystem.unlink(alias)
    await FileSystem.symlink(first, alias, 'file')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const matches = await fixtures.createPathMatcher(first)
    let retargeted = false
    setNodeHostBridgeLstatHook(async path => {
      if (pushArguments.length === 0 || !(await matches(path))) {
        return
      }
      setNodeHostBridgeLstatHook(undefined)
      await FileSystem.unlink(alias)
      await FileSystem.symlink(second, alias, 'file')
      retargeted = true
    })
    t.after(() => setNodeHostBridgeLstatHook(undefined))
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    assert.strictEqual(retargeted, true)
    for (const file of [first, second]) {
      const result = await actualExec(
        [
          'config',
          '--no-includes',
          '--file',
          file,
          '--get',
          'branch.master.merge',
        ],
        source.path
      )
      assert.strictEqual(result.exitCode, 1)
    }
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(pushArguments.length, 1)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
  })

  for (const branch of [
    'master',
    __WIN32__ ? "feature/quoted'branch" : 'feature/"quoted.branch',
  ]) {
    it(`round-trips literal tracking names through native stdin configuration for ${branch}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      if (branch !== 'master') {
        await fixtures.rawGit(source, ['branch', '--move', branch])
      }
      const remote = await remotes.addBareRemote(t, source, {
        name: __WIN32__ ? "o'rigin" : 'o"rigin',
        publish: false,
      })
      const configPath = join(source.path, '.git', 'config')
      const original = Buffer.concat([
        await FileSystem.readFile(configPath),
        Buffer.from('# Original comment without a final newline'),
      ])
      await FileSystem.writeFile(configPath, original)
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(outcome.refreshError, undefined)
      assert.strictEqual(
        await fixtures.rawGit(repository, [
          'config',
          '--local',
          '--get',
          '--',
          `branch.${branch}.remote`,
        ]),
        remote.name
      )
      assert.strictEqual(
        await fixtures.rawGit(repository, [
          'config',
          '--local',
          '--get',
          '--',
          `branch.${branch}.merge`,
        ]),
        remote.remoteRef
      )
      assert.ok(
        (await FileSystem.readFile(configPath))
          .subarray(0, original.length)
          .equals(original)
      )
      assert.strictEqual(
        allArguments.filter(
          args =>
            args[0] === 'config' &&
            args.includes('--get-all') &&
            args[args.indexOf('--file') + 1] === '-'
        ).length,
        2
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(repository), 2)
      assert.strictEqual(pushArguments.length, 1)
      assert.strictEqual(h.propose.mock.callCount(), 1)
    })
  }

  for (const after of ['push', 'fetch'] as const) {
    it(`never fetches or fast-forwards a checkout substituted after acknowledged ${after}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const replacement = join(
        await temporary.createTempDirectory(t),
        'replacement'
      )
      await FileSystem.cp(source.path, replacement, { recursive: true })
      const other = new repositories.Repository(replacement, 99, null, false)
      await fixtures.rawGit(other, ['branch', 'secondary', 'HEAD'])
      await fixtures.rawGit(other, [
        'config',
        '--local',
        '--',
        'branch.secondary.remote',
        'origin',
      ])
      await fixtures.rawGit(other, [
        'config',
        '--local',
        '--',
        'branch.secondary.merge',
        remote.remoteRef,
      ])
      const alias = join(await temporary.createTempDirectory(t), 'alias')
      const oldAlias = `${alias}-original`
      await FileSystem.symlink(source.path, alias, 'junction')
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(
        new repositories.Repository(alias, source.id, null, false)
      )
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      let retargeted = false
      const retarget = async () => {
        afterPush = undefined
        afterFollowUpFetch = undefined
        await FileSystem.rename(alias, oldAlias)
        await FileSystem.symlink(replacement, alias, 'junction')
        retargeted = true
      }
      if (after === 'push') {
        afterPush = retarget
      } else {
        afterFollowUpFetch = retarget
      }
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.strictEqual(retargeted, true)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.tip(source), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(source), 2)
      assert.strictEqual(await fixtures.tip(other), remote.originalTip)
      assert.strictEqual(
        await fixtures.rawGit(other, [
          'rev-parse',
          '--verify',
          'refs/remotes/origin/master',
        ]),
        remote.originalTip
      )
      assert.strictEqual(
        await fixtures.rawGit(other, [
          'rev-parse',
          '--verify',
          'refs/heads/secondary',
        ]),
        remote.originalTip
      )
      assert.strictEqual(
        await FileSystem.readFile(join(other.path, 'file'), 'utf8'),
        'before\n'
      )
      assert.strictEqual(pushArguments.length, 1)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    })
  }

  it('publishes and retries native configuration without creating or deleting private staging directories', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const directory = join(source.path, '.git', 'desktop-publication-unrelated')
    await FileSystem.mkdir(directory)
    await FileSystem.writeFile(
      join(directory, 'sentinel'),
      'Unrelated metadata stays\n'
    )
    let attemptedRemove = false
    denyRemove = async path => {
      if (
        typeof path === 'string' &&
        basename(path).startsWith('desktop-publication-')
      ) {
        attemptedRemove = true
        return true
      }
      afterStagingDirectoryCreation = async () => {
        throw new Error(
          'Publication must not create a private staging directory'
        )
      }
      directoryStatFailure = Object.assign(
        new Error('Publication must not stat a private staging directory'),
        { code: 'EIO' }
      )
      retainDirectoryStatFailure = true
      denyConfigKey = 'branch.master.merge'
      return false
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    assert.strictEqual(attemptedRemove, false)
    assert.strictEqual(failedDirectory, undefined)
    assert.strictEqual(stagingDirectoryCreations, 0)
    denyConfigKey = undefined
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
    assert.strictEqual(
      await FileSystem.readFile(join(directory, 'sentinel'), 'utf8'),
      'Unrelated metadata stays\n'
    )
    assert.strictEqual(attemptedRemove, false)
    assert.strictEqual(failedDirectory, undefined)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(repository), 2)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('expires a dangling original configuration target after acknowledgement without restoring it', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const config = join(source.path, '.git', 'config')
    const bytes = await FileSystem.readFile(config)
    const target = join(await temporary.createTempDirectory(t), 'config')
    await FileSystem.writeFile(target, bytes)
    await FileSystem.unlink(config)
    await FileSystem.symlink(target, config, 'file')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    afterPush = async () => {
      afterPush = undefined
      assert.strictEqual(
        (await FileSystem.lstat(config)).isSymbolicLink(),
        true
      )
      await FileSystem.unlink(target)
      assert.strictEqual(
        (await FileSystem.lstat(config)).isSymbolicLink(),
        true
      )
    }
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      await assert.rejects(FileSystem.lstat(target), { code: 'ENOENT' })
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      await FileSystem.writeFile(target, bytes)
    }
  })

  it('expires a dangling worktree configuration target before capture can strand publication', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await fixtures.rawGit(source, [
      'config',
      '--local',
      '--',
      'extensions.worktreeConfig',
      'true',
    ])
    const config = await fixtures.rawGit(source, [
      'rev-parse',
      '--path-format=absolute',
      '--git-path',
      'config.worktree',
    ])
    const target = join(await temporary.createTempDirectory(t), 'config')
    await FileSystem.writeFile(target, '')
    await FileSystem.symlink(target, config, 'file')
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    afterPush = async () => {
      afterPush = undefined
      assert.strictEqual(
        (await FileSystem.lstat(config)).isSymbolicLink(),
        true
      )
      await FileSystem.unlink(target)
    }
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      await assert.rejects(FileSystem.lstat(target), { code: 'ENOENT' })
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      await FileSystem.writeFile(target, '')
    }
  })

  for (const scope of ['global', 'system'] as const) {
    it(`expires publication when the acknowledged remote moves to ${scope} scope`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      const config = join(await temporary.createTempDirectory(t), 'config')
      await FileSystem.writeFile(config, '')
      const variable =
        scope === 'global' ? 'GIT_CONFIG_GLOBAL' : 'GIT_CONFIG_SYSTEM'
      const previous = [
        { name: variable, value: process.env[variable] },
        { name: 'GIT_CONFIG_NOSYSTEM', value: process.env.GIT_CONFIG_NOSYSTEM },
      ]
      process.env[variable] = config
      if (scope === 'system') {
        process.env.GIT_CONFIG_NOSYSTEM = '0'
      }
      try {
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
        const h = await harness.createAssistedCommitRunHarness(t)
        const repository = await h.register(source)
        h.dispatcher.setPushAfterAssistedCommit(repository, true)
        capturePushes = true
        afterPush = async path => {
          afterPush = undefined
          const moved = await actualExec(
            [
              'config',
              '--file',
              config,
              '--',
              'remote.origin.url',
              remote.path,
            ],
            path
          )
          assert.strictEqual(moved.exitCode, 0, moved.stderr)
          const removed = await actualExec(
            ['remote', 'remove', '--', 'origin'],
            path
          )
          assert.strictEqual(removed.exitCode, 0, removed.stderr)
        }
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'pushed')
        assert.ok(outcome.refreshError)
        const state = h.state(repository).changesState.assistedCommit
        assert.ok(state.kind === 'error')
        assert.strictEqual(state.retry, null)
        assert.strictEqual(await remote.tip(), outcome.result.head.sha)
        assert.strictEqual(await fixtures.count(repository), 2)
        assert.strictEqual(pushArguments.length, 1)
        h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
        assert.strictEqual(
          h.state(repository).changesState.assistedCommit.kind,
          'idle'
        )
      } finally {
        for (const { name, value } of previous) {
          if (value === undefined) {
            delete process.env[name]
          } else {
            process.env[name] = value
          }
        }
      }
    })
  }

  for (const cleanupFault of [false, true]) {
    it(`expires post-ACK unsupported includes only after owned cleanup, cleanup fault ${cleanupFault}`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      const changeInclude = async (path: string) => {
        if (cleanupFault) {
          assert.ok(
            (await FileSystem.lstat(join(path, '.git', 'config.lock'))).isFile()
          )
          await FileSystem.appendFile(
            join(path, '.git', 'config'),
            '\n[include]\n\tpath = "%(prefix)/desktop-assisted-missing-post-push-config"\n'
          )
          return
        }
        const result = await actualExec(
          [
            'config',
            '--local',
            '--',
            'include.path',
            '%(prefix)/desktop-assisted-missing-post-push-config',
          ],
          path
        )
        assert.strictEqual(result.exitCode, 0, result.stderr)
      }
      if (cleanupFault) {
        duringConfigWrite = async (_args, path) => {
          duringConfigWrite = undefined
          await changeInclude(path)
        }
        denyUnlink = await fixtures.createPathMatcher(
          join(repository.path, '.git', 'config.lock')
        )
      } else {
        afterPush = async path => {
          afterPush = undefined
          await changeInclude(path)
        }
      }
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, cleanupFault ? 'refresh' : null)
      if (cleanupFault) {
        denyUnlink = undefined
        await h.dispatcher.retryCopilotAssistedCommitRecovery(
          repository,
          state.runId
        )
        const settled = h.state(repository).changesState.assistedCommit
        assert.ok(settled.kind === 'error')
        assert.strictEqual(settled.retry, null)
      }
      await assert.rejects(
        FileSystem.lstat(join(repository.path, '.git', 'config.lock')),
        { code: 'ENOENT' }
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(repository), 2)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    })
  }

  for (const site of [
    'original-owner',
    'closing-owner',
    'closing-config',
  ] as const) {
    it(`retains acknowledged publication for retry after transient ${site} filesystem EIO`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      const target = await FileSystem.realpath(
        site === 'closing-config'
          ? join(repository.path, '.git', 'config')
          : repository.path
      )
      syncFailure = Object.assign(
        new Error('Synthetic transient metadata EIO'),
        { code: 'EIO' }
      )
      const inject = () => {
        denySyncStat = path =>
          typeof path === 'string' &&
          FileSystemSync.realpathSync(path) === target
      }
      if (site === 'original-owner') {
        afterPush = async () => {
          afterPush = undefined
          inject()
        }
      } else {
        duringConfigWrite = async () => {
          duringConfigWrite = undefined
          inject()
        }
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.ok(
        errors
          .assistedCommitErrorCauses(outcome.refreshError)
          .includes(syncFailure)
      )
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, 'refresh')
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'error'
      )
      await h.dispatcher.retryCopilotAssistedCommitRecovery(
        repository,
        state.runId
      )
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
      assert.strictEqual(
        await fixtures.rawGit(repository, [
          'config',
          '--get',
          'branch.master.merge',
        ]),
        remote.remoteRef
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(await fixtures.count(repository), 2)
      assert.strictEqual(pushArguments.length, 1)
      assert.strictEqual(h.propose.mock.callCount(), 1)
    })
  }

  for (const sourceKind of ['global', 'system'] as const) {
    it(`never certifies a stale ${sourceKind} include graph before a late destination rewrite`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source)
      const replacement = await remotes.addBareRemote(t, source, {
        name: 'replacement',
        publish: false,
      })
      await fixtures.rawGit(source, [
        'push',
        '--',
        'replacement',
        remote.branchRef,
      ])
      const root = join(await temporary.createTempDirectory(t), 'root-config')
      const included = join(
        await temporary.createTempDirectory(t),
        'new-include'
      )
      await FileSystem.writeFile(root, '')
      await FileSystem.writeFile(included, '')
      const variable =
        sourceKind === 'global' ? 'GIT_CONFIG_GLOBAL' : 'GIT_CONFIG_SYSTEM'
      const previous = [
        { name: variable, value: process.env[variable] },
        { name: 'GIT_CONFIG_NOSYSTEM', value: process.env.GIT_CONFIG_NOSYSTEM },
      ]
      process.env[variable] = root
      if (sourceKind === 'system') {
        process.env.GIT_CONFIG_NOSYSTEM = '0'
      }
      try {
        await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
        const h = await harness.createAssistedCommitRunHarness(t)
        const repository = await h.register(source)
        h.dispatcher.setPushAfterAssistedCommit(repository, true)
        let added = false
        afterConfigurationRead = async () => {
          if (
            added ||
            h.state(repository).changesState.assistedCommit.kind !==
              'preparing-push'
          ) {
            return
          }
          added = true
          afterConfigurationRead = undefined
          const result = await actualExec(
            ['config', '--file', root, '--', 'include.path', included],
            repository.path
          )
          assert.strictEqual(result.exitCode, 0, result.stderr)
        }
        afterPinnedLookup = async () => {
          afterPinnedLookup = undefined
          const result = await actualExec(
            [
              'config',
              '--file',
              included,
              '--',
              `url.${replacement.path}.insteadOf`,
              remote.path,
            ],
            repository.path
          )
          assert.strictEqual(result.exitCode, 0, result.stderr)
        }
        capturePushes = true
        const outcome = await h.dispatcher.createCopilotAssistedCommits(
          repository,
          h.request(repository)
        )
        assert.ok(outcome.kind === 'push-error')
        assert.strictEqual(outcome.attempted, false)
        assert.strictEqual(added, true)
        assert.strictEqual(pushArguments.length, 0)
        assert.strictEqual(await remote.tip(), remote.originalTip)
        assert.strictEqual(await replacement.tip(), replacement.originalTip)
        assert.strictEqual(await fixtures.count(repository), 2)
      } finally {
        for (const { name, value } of previous) {
          if (value === undefined) {
            delete process.env[name]
          } else {
            process.env[name] = value
          }
        }
      }
    })
  }

  it('rejects a newly discovered nested include before installing stale publication tracking', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const root = join(await temporary.createTempDirectory(t), 'root-config')
    const first = join(await temporary.createTempDirectory(t), 'first-include')
    const nested = join(
      await temporary.createTempDirectory(t),
      'nested-include'
    )
    await FileSystem.writeFile(root, '')
    await FileSystem.writeFile(first, '')
    await FileSystem.writeFile(nested, '')
    const include = await actualExec(
      ['config', '--file', root, '--', 'include.path', first],
      source.path
    )
    assert.strictEqual(include.exitCode, 0, include.stderr)
    const previous = process.env.GIT_CONFIG_GLOBAL
    process.env.GIT_CONFIG_GLOBAL = root
    try {
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      let added = false
      afterConfigurationRead = async () => {
        if (pushArguments.length === 0) {
          return
        }
        afterConfigurationRead = undefined
        added = true
        const result = await actualExec(
          ['config', '--file', first, '--', 'include.path', nested],
          repository.path
        )
        assert.strictEqual(result.exitCode, 0, result.stderr)
      }
      duringConfigWrite = async () => {
        duringConfigWrite = undefined
        const result = await actualExec(
          [
            'config',
            '--file',
            nested,
            '--',
            'branch.master.merge',
            'refs/heads/external',
          ],
          repository.path
        )
        assert.strictEqual(result.exitCode, 0, result.stderr)
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.strictEqual(added, true)
      const tracking = await actualExec(
        ['config', '--local', '--get', 'branch.master.merge'],
        repository.path
      )
      assert.strictEqual(tracking.exitCode, 1)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      if (previous === undefined) {
        delete process.env.GIT_CONFIG_GLOBAL
      } else {
        process.env.GIT_CONFIG_GLOBAL = previous
      }
    }
  })

  it('expires unsupported symbolic post-push branch identity without holding publication forever', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    afterPush = async path => {
      afterPush = undefined
      const result = await actualExec(
        ['update-ref', 'refs/heads/alias-target', 'HEAD'],
        path
      )
      assert.strictEqual(result.exitCode, 0, result.stderr)
      const alias = await actualExec(
        ['symbolic-ref', remote.branchRef, 'refs/heads/alias-target'],
        path
      )
      assert.strictEqual(alias.exitCode, 0, alias.stderr)
    }
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.tip(repository), outcome.result.head.sha)
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'symbolic-ref',
        '--no-recurse',
        remote.branchRef,
      ]),
      'refs/heads/alias-target'
    )
    assert.strictEqual(pushArguments.length, 1)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
  })

  it('expires an unavailable post-push HEAD after cleanup without restoring deleted metadata', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const head = await fixtures.rawGit(source, [
      'rev-parse',
      '--path-format=absolute',
      '--git-path',
      'HEAD',
    ])
    const original = await FileSystem.readFile(head)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    afterPush = async () => {
      afterPush = undefined
      await FileSystem.unlink(head)
    }
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(
        state.retry,
        null,
        errors
          .assistedCommitErrorCauses(state.error)
          .map(error =>
            error instanceof Error
              ? `${error.stack ?? error.message}\n${
                  'args' in error ? JSON.stringify(error.args) : ''
                }`
              : String(error)
          )
          .join('\n')
      )
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      await assert.rejects(FileSystem.lstat(head), { code: 'ENOENT' })
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      await FileSystem.writeFile(head, original)
    }
  })

  it('stages publication into the real owned config despite a shadow introduced after native push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const shadow = join(await temporary.createTempDirectory(t), 'config-shadow')
    await FileSystem.writeFile(shadow, '')
    const config = await actualExec(
      ['config', '--file', shadow, '--', 'remote.origin.url', remote.path],
      source.path
    )
    assert.strictEqual(config.exitCode, 0, config.stderr)
    const previous = process.env.GIT_CONFIG
    capturePushes = true
    afterPush = async () => {
      afterPush = undefined
      process.env.GIT_CONFIG = shadow
    }
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.strictEqual(outcome.refreshError, undefined)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      const actual = await actualExec(
        ['config', '--local', '--get', 'branch.master.merge'],
        repository.path,
        { env: { GIT_CONFIG: undefined } }
      )
      assert.strictEqual(actual.exitCode, 0, actual.stderr)
      assert.strictEqual(actual.stdout.trim(), remote.remoteRef)
      const untouched = await actualExec(
        ['config', '--file', shadow, '--get', 'branch.master.merge'],
        repository.path,
        { env: { GIT_CONFIG: undefined } }
      )
      assert.strictEqual(untouched.exitCode, 1)
      assert.strictEqual(pushArguments.length, 1)
      assert.strictEqual(await fixtures.count(repository), 2)
    } finally {
      if (previous === undefined) {
        delete process.env.GIT_CONFIG
      } else {
        process.env.GIT_CONFIG = previous
      }
    }
  })

  for (const packed of [false, true]) {
    it(`expires publication intent when an external ref writer changes the ${
      packed ? 'packed' : 'loose'
    } branch during staging`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      const tree = await fixtures.rawGit(source, [
        'rev-parse',
        '--verify',
        'HEAD^{tree}',
      ])
      const external = await fixtures.rawGit(source, [
        'commit-tree',
        tree,
        '-p',
        remote.originalTip,
        '-m',
        'External independent tip',
      ])
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      capturePushes = true
      if (packed) {
        afterPush = async path => {
          afterPush = undefined
          const result = await actualExec(['pack-refs', '--all'], path)
          assert.strictEqual(result.exitCode, 0, result.stderr)
        }
      }
      let changed = false
      duringConfigWrite = async (_args, path) => {
        if (changed) {
          return
        }
        changed = true
        const result = await actualExec(
          ['update-ref', remote.branchRef, external],
          path
        )
        assert.strictEqual(result.exitCode, 0, result.stderr)
        if (packed) {
          const pack = await actualExec(['pack-refs', '--all'], path)
          assert.strictEqual(pack.exitCode, 0, pack.stderr)
        }
      }
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.strictEqual(changed, true)
      assert.strictEqual(await fixtures.tip(repository), external)
      const accepted = outcome.result.head.sha
      assert.ok(accepted !== null)
      assert.strictEqual(await remote.tip(), accepted)
      assert.strictEqual(
        await fixtures.rawGit(repository, ['cat-file', '-t', accepted]),
        'commit'
      )
      assert.strictEqual(
        (
          await actualExec(
            ['config', '--local', '--get', 'branch.master.merge'],
            repository.path
          )
        ).exitCode,
        1
      )
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    })
  }

  it('cleans retained publication resources under frozen admission before expiring broken linked routing', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const linked = join(await temporary.createTempDirectory(t), 'linked')
    await fixtures.rawGit(source, [
      'worktree',
      'add',
      '-b',
      'waiting-linked',
      '--',
      linked,
      'HEAD',
    ])
    const linkedRepository = new repositories.Repository(
      linked,
      source.id,
      null,
      false
    )
    const directory = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--absolute-git-dir',
    ])
    const common = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--path-format=absolute',
      '--git-common-dir',
    ])
    const missing = join(
      await temporary.createTempDirectory(t),
      'missing-common'
    )
    await FileSystem.writeFile(join(linked, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(linkedRepository)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    denyConfigKey = 'branch.waiting-linked.merge'
    denyUnlink = await fixtures.createPathMatcher(join(common, 'config.lock'))
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    await FileSystem.lstat(join(common, 'config.lock'))
    denyConfigKey = undefined
    denyUnlink = undefined
    await FileSystem.writeFile(join(directory, 'commondir'), `${missing}\n`)
    try {
      await h.dispatcher.retryCopilotAssistedCommitRecovery(
        repository,
        state.runId
      )
      await assert.rejects(FileSystem.lstat(join(common, 'config.lock')), {
        code: 'ENOENT',
      })
      const settled = h.state(repository).changesState.assistedCommit
      assert.ok(settled.kind === 'error')
      assert.strictEqual(settled.retry, null)
      assert.strictEqual(
        await remote.read([
          'rev-parse',
          '--verify',
          'refs/heads/waiting-linked',
        ]),
        outcome.result.head.sha
      )
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      await FileSystem.writeFile(join(directory, 'commondir'), `${common}\n`)
      const current = h.state(repository).changesState.assistedCommit
      if (current.kind === 'error' && current.retry !== null) {
        await h.dispatcher.retryCopilotAssistedCommitRecovery(
          repository,
          state.runId
        )
      }
    }
  })

  it('expires missing-remote publication setup after acknowledged push without a permanent refresh lock', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    afterPush = async path => {
      afterPush = undefined
      const result = await actualExec(
        ['remote', 'remove', '--', 'origin'],
        path
      )
      assert.strictEqual(result.exitCode, 0, result.stderr)
    }
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, null)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(await fixtures.count(repository), 2)
    assert.strictEqual(pushArguments.length, 1)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
  })

  for (const initial of ['empty', 'missing'] as const) {
    it(`rejects a publication tracking choice concurrently installed in ${initial} worktree configuration`, async t => {
      reset()
      t.after(reset)
      nodeBackend(t)
      const source = await fixtures.seed(t, { file: 'before\n' })
      const remote = await remotes.addBareRemote(t, source, { publish: false })
      await fixtures.rawGit(source, [
        'config',
        '--local',
        '--',
        'extensions.worktreeConfig',
        'true',
      ])
      const worktreeConfig = await fixtures.rawGit(source, [
        'rev-parse',
        '--path-format=absolute',
        '--git-path',
        'config.worktree',
      ])
      if (initial === 'empty') {
        await FileSystem.writeFile(worktreeConfig, '')
      }
      await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
      const h = await harness.createAssistedCommitRunHarness(t)
      const repository = await h.register(source)
      h.dispatcher.setPushAfterAssistedCommit(repository, true)
      let changed = false
      duringConfigWrite = async (_args, path) => {
        if (changed) {
          return
        }
        changed = true
        const result = await actualExec(
          [
            'config',
            '--worktree',
            '--',
            'branch.master.merge',
            'refs/heads/external',
          ],
          path
        )
        assert.strictEqual(result.exitCode, 0, result.stderr)
      }
      capturePushes = true
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.strictEqual(changed, true)
      assert.strictEqual(
        (
          await actualExec(
            ['config', '--local', '--get', 'branch.master.merge'],
            repository.path
          )
        ).exitCode,
        1
      )
      assert.strictEqual(
        await fixtures.rawGit(repository, [
          'config',
          '--get-all',
          'branch.master.merge',
        ]),
        'refs/heads/external'
      )
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      assert.strictEqual(await remote.tip(), outcome.result.head.sha)
      assert.strictEqual(pushArguments.length, 1)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    })
  }

  it('never installs publication metadata into substituted linked-worktree common resources', async t => {
    reset()
    t.after(reset)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    const linked = join(await temporary.createTempDirectory(t), 'linked')
    await fixtures.rawGit(source, [
      'worktree',
      'add',
      '-b',
      'published-linked',
      '--',
      linked,
      'HEAD',
    ])
    const linkedRepository = new repositories.Repository(
      linked,
      source.id,
      null,
      false
    )
    const directory = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--absolute-git-dir',
    ])
    const common = await fixtures.rawGit(linkedRepository, [
      'rev-parse',
      '--path-format=absolute',
      '--git-common-dir',
    ])
    const replacement = join(
      await temporary.createTempDirectory(t),
      'copied-common'
    )
    await FileSystem.writeFile(join(linked, 'file'), 'after\n')
    nodeBackend(t)
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(linkedRepository)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    let substituted = false
    beforePublicationConfig = async () => {
      beforePublicationConfig = undefined
      await FileSystem.cp(common, replacement, { recursive: true })
      await FileSystem.writeFile(
        join(directory, 'commondir'),
        `${replacement}\n`
      )
      substituted = true
    }
    try {
      const outcome = await h.dispatcher.createCopilotAssistedCommits(
        repository,
        h.request(repository)
      )
      assert.ok(outcome.kind === 'pushed')
      assert.ok(outcome.refreshError)
      assert.strictEqual(substituted, true)
      const tracking = await actualExec(
        [
          'config',
          '--no-includes',
          '--file',
          join(replacement, 'config'),
          '--get',
          'branch.published-linked.merge',
        ],
        repository.path
      )
      assert.strictEqual(tracking.exitCode, 1)
      assert.strictEqual(
        await remote.read([
          'rev-parse',
          '--verify',
          'refs/heads/published-linked',
        ]),
        outcome.result.head.sha
      )
      assert.strictEqual(await fixtures.count(repository), 2)
      const state = h.state(repository).changesState.assistedCommit
      assert.ok(state.kind === 'error')
      assert.strictEqual(state.retry, null)
      h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
      assert.strictEqual(
        h.state(repository).changesState.assistedCommit.kind,
        'idle'
      )
    } finally {
      if (substituted) {
        await FileSystem.writeFile(join(directory, 'commondir'), `${common}\n`)
      }
    }
  })

  it('retains publication cleanup before expiring a stale tracking choice', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    duringConfigWrite = async (_args, path) => {
      duringConfigWrite = undefined
      assert.ok(
        (await FileSystem.lstat(join(path, '.git', 'config.lock'))).isFile()
      )
      await FileSystem.appendFile(
        join(path, '.git', 'config'),
        '\n[branch "master"]\n\tmerge = refs/heads/external\n'
      )
    }
    denyUnlink = await fixtures.createPathMatcher(
      join(repository.path, '.git', 'config.lock')
    )
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'error'
    )
    assert.strictEqual(await fixtures.count(repository), 2)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)

    denyUnlink = undefined
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    const settled = h.state(repository).changesState.assistedCommit
    assert.ok(settled.kind === 'error')
    assert.strictEqual(settled.retry, null)
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--get',
        'branch.master.merge',
      ]),
      'refs/heads/external'
    )
    assert.strictEqual(pushArguments.length, 1)
    assert.strictEqual(await fixtures.count(repository), 2)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
    await assert.rejects(
      FileSystem.lstat(join(repository.path, '.git', 'config.lock')),
      { code: 'ENOENT' }
    )
  })

  it('publishes upstream as one owned configuration transaction, excluding an external writer between keys', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    let attempted = false
    duringConfigWrite = async (_args, path) => {
      if (attempted) {
        return
      }
      attempted = true
      const change = await actualExec(
        [
          'config',
          '--local',
          '--',
          'branch.master.merge',
          'refs/heads/external',
        ],
        path
      )
      assert.notStrictEqual(change.exitCode, 0)
      assert.match(change.stderr.toString(), /lock|File exists/)
    }
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(outcome.refreshError, undefined)
    assert.strictEqual(attempted, true)
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--get',
        'branch.master.remote',
      ]),
      'origin'
    )
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--get',
        'branch.master.merge',
      ]),
      remote.remoteRef
    )
    assert.strictEqual(await remote.tip(), await fixtures.tip(repository))
  })

  it('expires stale publication setup without locking acknowledged history or pushing again', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    beforePush = async path => {
      beforePush = undefined
      const commit = await actualExec(
        ['commit', '--allow-empty', '-m', 'External after entry'],
        path
      )
      assert.strictEqual(commit.exitCode, 0)
    }
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, null)
    const fullTip = await fixtures.tip(repository)
    h.dispatcher.dismissCopilotAssistedCommitError(repository, state.runId)
    assert.strictEqual(
      h.state(repository).changesState.assistedCommit.kind,
      'idle'
    )
    assert.strictEqual(await fixtures.count(repository), 3)
    assert.strictEqual(await fixtures.tip(repository), fullTip)
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.strictEqual(pushArguments.length, 1)
  })

  it('keeps frozen destination URLs out of every native argument, including offline verification', async t => {
    reset()
    t.after(reset)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(
      allArguments.some(args => args.some(arg => arg === remote.path)),
      false
    )
    assert.strictEqual(await remote.tip(), await fixtures.tip(repository))
  })

  it('retry refresh completes failed staged upstream setup without a partial installation or another native push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source, { publish: false })
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    denyConfigKey = 'branch.master.merge'
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.ok(outcome.refreshError)
    assert.strictEqual(await remote.tip(), await fixtures.tip(repository))
    assert.strictEqual(
      (
        await actualExec(
          ['config', '--get', 'branch.master.remote'],
          repository.path
        )
      ).exitCode,
      1
    )
    assert.strictEqual(
      (
        await actualExec(
          ['config', '--get', 'branch.master.merge'],
          repository.path
        )
      ).exitCode,
      1
    )
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'refresh')
    denyConfigKey = undefined
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    assert.strictEqual(
      await fixtures.rawGit(repository, [
        'config',
        '--get',
        'branch.master.merge',
      ]),
      remote.remoteRef
    )
    assert.strictEqual(pushArguments.length, 1)
    assert.strictEqual(h.propose.mock.callCount(), 1)
    assert.strictEqual(await fixtures.count(repository), 2)
  })

  it('keeps every local SHA after an unknown remote response and retries only the same certified push', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const transportFailure = new Error(
      'Synthetic response lost after remote acceptance'
    )
    capturePushes = true
    reportPushFailure = transportFailure
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'push-error')
    assert.ok(
      errors.assistedCommitErrorCauses(outcome.error).includes(transportFailure)
    )
    const fullTip = await fixtures.tip(repository)
    assert.strictEqual(await remote.tip(), fullTip)
    assert.strictEqual(await fixtures.count(repository), 2)
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'push-error')
    const retry = await h.dispatcher.retryCopilotAssistedCommitPush(
      repository,
      state.runId
    )
    assert.ok(retry?.kind === 'pushed')
    assert.strictEqual(await remote.tip(), fullTip)
    assert.strictEqual(await fixtures.tip(repository), fullTip)
    assert.strictEqual(await fixtures.count(repository), 2)
    assert.strictEqual(h.propose.mock.callCount(), 1)
    assert.strictEqual(h.commits.mock.callCount(), 1)
    assert.strictEqual(pushArguments.length, 2)
    for (const args of pushArguments) {
      assert.ok(args.includes('--no-force'))
      assert.ok(args.includes('--no-follow-tags'))
      assert.ok(args.includes('--recurse-submodules=no'))
      assert.strictEqual(args.includes('--force-with-lease'), false)
      assert.strictEqual(args.includes('--no-verify'), false)
      assert.deepStrictEqual(args.slice(args.indexOf('--')), [
        '--',
        'origin',
        `${fullTip}:${remote.remoteRef}`,
      ])
    }
  })

  it('never broadens the source to an external commit made after entry and never rolls it back', async t => {
    reset()
    t.after(reset)
    nodeBackend(t)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    capturePushes = true
    beforePush = async path => {
      beforePush = undefined
      h.cancel(repository)
      const commit = await actualExec(
        ['commit', '--allow-empty', '-m', 'External after entry'],
        path
      )
      assert.strictEqual(commit.exitCode, 0, commit.stderr.toString())
    }
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'pushed')
    assert.strictEqual(await remote.tip(), outcome.result.head.sha)
    assert.notStrictEqual(
      await fixtures.tip(repository),
      outcome.result.head.sha
    )
    assert.strictEqual(await fixtures.count(repository), 3)
    assert.strictEqual(
      await fixtures.rawGit(repository, ['show', '-s', '--format=%s', 'HEAD']),
      'External after entry'
    )
    assert.strictEqual(h.commits.mock.callCount(), 1)
    assert.strictEqual(h.propose.mock.callCount(), 1)
  })

  it('never enters native push while pre-push fence cleanup retains ownership; retry is rollback only', async t => {
    reset()
    t.after(reset)
    const source = await fixtures.seed(t, { file: 'before\n' })
    const remote = await remotes.addBareRemote(t, source)
    await FileSystem.writeFile(join(source.path, 'file'), 'after\n')
    const h = await harness.createAssistedCommitRunHarness(t)
    const repository = await h.register(source)
    h.dispatcher.setPushAfterAssistedCommit(repository, true)
    const isHeadLock = await fixtures.createPathMatcher(
      join(repository.path, '.git', 'HEAD.lock')
    )
    let latched = false
    denyUnlink = async path => {
      if (!(await isHeadLock(path))) {
        return false
      }
      latched ||=
        h.state(repository).changesState.assistedCommit.kind ===
        'preparing-push'
      return latched
    }
    capturePushes = true
    const outcome = await h.dispatcher.createCopilotAssistedCommits(
      repository,
      h.request(repository)
    )
    assert.ok(outcome.kind === 'error')
    const state = h.state(repository).changesState.assistedCommit
    assert.ok(state.kind === 'error')
    assert.strictEqual(state.retry, 'recovery')
    assert.strictEqual(pushArguments.length, 0)
    assert.strictEqual(await remote.tip(), remote.originalTip)
    assert.strictEqual(h.commits.mock.callCount(), 0)
    assert.strictEqual(
      await h.dispatcher.retryCopilotAssistedCommitPush(
        repository,
        state.runId
      ),
      undefined
    )
    denyUnlink = undefined
    await h.dispatcher.retryCopilotAssistedCommitRecovery(
      repository,
      state.runId
    )
    assert.strictEqual(await fixtures.count(repository), 1)
    assert.strictEqual(await remote.tip(), remote.originalTip)
    assert.strictEqual(pushArguments.length, 0)
    assert.strictEqual(h.propose.mock.callCount(), 1)
  })
})
