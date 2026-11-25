import {
  backend,
  BackendError,
  type FetchProgressChunk,
} from '../../backend/client'
import { isRecord } from '../../lib/is-record'
import { getHTMLURL } from '../../lib/api'
import {
  canonicalMutationPath,
  protectAssistedCommitResources,
  withRepositoryGitResourceMutation,
} from '../../lib/git/repository-operation'

/** Preserve Enterprise ports when binding the selected account's host. */
export function nativeCloneAccountHost(endpoint: string): string {
  return new URL(getHTMLURL(endpoint)).host
}

/** Render byte paths without silently replacing non-UTF-8 bytes. */
function displayPath(value: unknown): string | null {
  if (typeof value === 'string') {
    return value
  }
  if (
    !isRecord(value) ||
    !Array.isArray(value.bytes) ||
    value.bytes.length > 16384 ||
    !value.bytes.every(
      (byte: unknown) =>
        typeof byte === 'number' &&
        Number.isInteger(byte) &&
        byte >= 0 &&
        byte <= 255
    )
  ) {
    return null
  }
  return `Bytes: ${value.bytes
    .map((byte: number) => `\\x${byte.toString(16).padStart(2, '0')}`)
    .join('')}`
}

export interface INativeCloneRecovery {
  readonly id: string
  readonly destination: string
  readonly recoverable: boolean
  readonly mutationPath: string | null
  readonly stagePath: string | null
}

/** List pending clones without exposing their stored URLs or account data. */
export async function listNativeCloneRecoveries(): Promise<
  ReadonlyArray<INativeCloneRecovery>
> {
  let result
  try {
    result = await backend().listNativeCloneRecoveries()
  } catch (error) {
    if (error instanceof BackendError && error.code === 'invalid_response') {
      throw new Error('Unable to read clone recovery history.')
    }
    throw error
  }
  return result.map(record => {
    const destination = displayPath(record.destination)
    if (destination === null) {
      throw new Error('Unable to read clone recovery history.')
    }
    return {
      id: record.id,
      destination,
      recoverable:
        record.recoverable &&
        typeof record.destination === 'string' &&
        typeof record.stage === 'string',
      mutationPath:
        typeof record.destination === 'string' ? record.destination : null,
      stagePath: typeof record.stage === 'string' ? record.stage : null,
    }
  })
}

/** Decode only the fixed progress labels emitted by the native clone service. */
export function nativeCloneProgress(chunk: FetchProgressChunk): string | null {
  try {
    const value = atob(chunk.data).trim()
    return /^(?:remote: )?(?:Enumerating objects|Counting objects|Compressing objects|Receiving objects|Resolving deltas|Checking out files|Updating files|Filtering content): (?:100|[0-9]{1,2})% \((?:100|[0-9]{1,2})\/100\)$/.test(
      value
    )
      ? value
      : null
  } catch {
    return null
  }
}

/** Run an explicitly selected nonrecursive clone, preserving its saved account. */
export async function nativeClone(
  url: string,
  path: string,
  account: {
    readonly host: string
    readonly login: string
    readonly id: number
  },
  defaultBranch: string | undefined,
  onProgress: (text: string) => void
): Promise<{ readonly path: string; readonly staging: string }> {
  return withRepositoryGitResourceMutation([path], async () => {
    const canonical = await canonicalMutationPath(path)
    return withRepositoryGitResourceMutation([canonical], async () => {
      try {
        return await nativeCloneReserved(
          url,
          path,
          account,
          defaultBranch,
          onProgress
        )
      } catch (error) {
        retainUncertainCloneResources(error, [path, canonical])
        throw error
      }
    })
  })
}

/** A lost transport cannot confirm that the host has stopped mutating files. */
function retainUncertainCloneResources(
  error: unknown,
  paths: ReadonlyArray<string>
): void {
  if (
    error instanceof BackendError &&
    ([
      'disconnected',
      'protocol',
      'unauthorized',
      'cancelled',
      'ETIMEDOUT',
      'timeout',
    ].includes(error.code) ||
      error.message.startsWith('IPC timeout:'))
  ) {
    protectAssistedCommitResources(paths[0], paths)
    throw new Error(
      'Clone completion could not be confirmed. The destination is protected only in this session, and recovery is unavailable in this session. Inspect the destination and any staging directories.'
    )
  }
}

async function nativeCloneReserved(
  url: string,
  path: string,
  account: {
    readonly host: string
    readonly login: string
    readonly id: number
  },
  defaultBranch: string | undefined,
  onProgress: (text: string) => void
): Promise<{ readonly path: string; readonly staging: string }> {
  const client = backend()
  const options = {
    url,
    path,
    account,
    default_branch: defaultBranch,
  }
  let result
  try {
    result = await client.cloneSelectedRepository(options, {
      deadlineMs: 3_600_000,
      onProgress: chunk => {
        const text = nativeCloneProgress(chunk)
        if (text !== null) {
          onProgress(text)
        }
      },
    })
  } catch (error) {
    if (error instanceof BackendError && error.code === 'invalid_response') {
      throw new Error(
        'Clone may have completed; inspect the destination before retrying.'
      )
    }
    throw error
  }
  const savedPath = displayPath(result.record.path)
  const staging = displayPath(result.retained_staging)
  if (savedPath === null || staging === null) {
    throw new Error(
      'Clone may have completed; inspect the destination before retrying.'
    )
  }
  return { path: savedPath, staging }
}

/** Resume a ready clone; callers must explicitly open it after recovery. */
export async function recoverNativeClone(
  id: string,
  destination: string,
  stage: string
): Promise<{ readonly path: string; readonly staging: string }> {
  return withRepositoryGitResourceMutation([destination, stage], async () => {
    const canonical = await Promise.all([
      canonicalMutationPath(destination),
      canonicalMutationPath(stage),
    ])
    return withRepositoryGitResourceMutation(canonical, async () => {
      try {
        return await recoverNativeCloneReserved(id)
      } catch (error) {
        retainUncertainCloneResources(error, [destination, stage, ...canonical])
        throw error
      }
    })
  })
}

async function recoverNativeCloneReserved(
  id: string
): Promise<{ readonly path: string; readonly staging: string }> {
  const client = backend()
  let result
  try {
    result = await client.recoverNativeClone(id, { deadlineMs: 3_600_000 })
  } catch (error) {
    if (error instanceof BackendError && error.code === 'invalid_response') {
      throw new Error(
        'Recovery may have completed; inspect the destination before retrying.'
      )
    }
    throw error
  }
  const path = displayPath(result.path)
  const staging = displayPath(result.retained_staging)
  if (path === null || staging === null) {
    throw new Error(
      'Recovery may have completed; inspect the destination before retrying.'
    )
  }
  return { path, staging }
}
