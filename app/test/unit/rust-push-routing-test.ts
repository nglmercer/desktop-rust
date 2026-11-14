import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  push: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
  detached: false,
  without: vi.fn((run: () => void) => {
    const previous = mocks.detached
    mocks.detached = true
    try {
      run()
    } finally {
      mocks.detached = previous
    }
  }),
  environment: vi.fn(async () => ({})),
  proxy: vi.fn(
    async (): Promise<Record<string, string> | undefined> => undefined
  ),
  convert: vi.fn(() =>
    Object.assign(new Error('classified failure'), {
      result: { exitCode: 1, gitError: null },
    })
  ),
  PreviewUnsupported: class extends Error {},
  BackendError: class extends Error {
    constructor(public code: string, message: string) {
      super(message)
    }
  },
  GitError: class extends Error {
    public readonly result: unknown
    constructor(result: { gitErrorDescription: string }) {
      super(result.gitErrorDescription)
      this.result = result
    }
  },
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    pushRepositoryRemote: mocks.push,
  }),
  FetchPreviewUnsupported: mocks.PreviewUnsupported,
  BackendError: mocks.BackendError,
}))
vi.mock('../../src/lib/git/core', () => ({
  git: mocks.git,
  GitError: mocks.GitError,
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: mocks.environment,
  envForProxy: mocks.proxy,
}))
vi.mock('../../src/lib/git/fetch-error', () => ({
  fetchFailureToGitError: mocks.convert,
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
  withoutRepositoryGitAccess: mocks.without,
}))

import {
  isNativePushOutcomeUncertain,
  push,
  suppressUnsafePushRetry,
} from '../../src/lib/git/push'
import { ErrorWithMetadata } from '../../src/lib/error-with-metadata'
import {
  pushNeedsPullHandler,
  samlReauthRequired,
} from '../../src/ui/dispatcher/error-handlers'
import type { Dispatcher } from '../../src/ui/dispatcher'

const repository = { path: '/repo' } as Repository
const remote = { name: 'origin', url: 'https://example.invalid/owner/repo' }

describe('Rust push routing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    for (const key of [
      'http_proxy',
      'https_proxy',
      'HTTPS_PROXY',
      'all_proxy',
      'ALL_PROXY',
    ])
      vi.stubEnv(key, undefined)
    mocks.transport = 'ws'
    mocks.push.mockReset().mockResolvedValue(undefined)
    mocks.proxy.mockReset().mockResolvedValue(undefined)
    mocks.git.mockResolvedValue(undefined)
  })
  afterEach(() => {
    vi.unstubAllEnvs()
    Reflect.deleteProperty(window, '__desktop')
  })

  it.each(['ws', 'wry'])(
    'pushes the selected branch and tags via %s with progress',
    async transport => {
      mocks.transport = transport
      Object.defineProperty(window, '__desktop', {
        configurable: true,
        value: { invokeWithProgress: vi.fn() },
      })
      mocks.push.mockImplementation(
        async (_path, _request, onProgress, checkDestinations) => {
          if (checkDestinations)
            await checkDestinations(['https://example.invalid/owner/push'])
          onProgress({
            stream: 'stderr',
            data: btoa('Writing objects: 50% (1/2)\r'),
          })
        }
      )
      const progress = vi.fn()
      await push(
        repository,
        remote,
        'main',
        'main',
        ['v1'],
        { forceWithLease: true },
        progress
      )
      expect(mocks.scope).toHaveBeenCalledWith(
        '/repo',
        'mutation',
        expect.any(Function),
        true
      )
      expect(mocks.push).toHaveBeenCalledWith(
        '/repo',
        {
          remote: 'origin',
          expected_url: remote.url,
          local_branch: 'main',
          remote_branch: 'main',
          tags: ['v1'],
          force_with_lease: true,
          no_verify: false,
          report_progress: true,
          require_hook_callbacks: false,
          interactive_hook_decision: false,
        },
        expect.any(Function),
        transport === 'wry' ? expect.any(Function) : undefined,
        expect.any(Function),
        undefined,
        undefined
      )
      expect(progress).toHaveBeenCalledWith(
        expect.objectContaining({
          description: expect.stringContaining('Writing objects'),
          remote: 'origin',
        })
      )
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it.each(['ws', 'wry'])(
    'never retries a failed or uncertain %s push via the legacy bridge',
    async transport => {
      mocks.transport = transport
      mocks.push
        .mockResolvedValueOnce({ exitCode: 1, errorKind: null })
        .mockRejectedValueOnce(new Error('outcome uncertain'))
      await expect(
        push(repository, remote, 'main', 'main', [])
      ).rejects.toThrow('Some destinations or tags may have been updated')
      await expect(
        push(repository, remote, 'main', 'main', [])
      ).rejects.toThrow('outcome uncertain')
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )

  it('falls back before mutation if a Wry destination needs a proxy', async () => {
    mocks.transport = 'wry'
    mocks.proxy.mockResolvedValue({ https_proxy: 'http://proxy.invalid' })
    mocks.push.mockImplementation(
      async (_path, _request, _onProgress, checkDestinations) => {
        await checkDestinations(['https://example.invalid/alternate'])
      }
    )
    await push(repository, remote, 'main', null, [])
    expect(mocks.git).toHaveBeenCalledTimes(1)
    expect(mocks.proxy).toHaveBeenCalledWith(
      'https://example.invalid/alternate'
    )
  })

  it('keeps a partial push warning visible instead of opening a pull or authentication dialog', async () => {
    mocks.push.mockResolvedValueOnce({
      exitCode: 1,
      errorKind: 'push_non_fast_forward',
    })
    const result = await push(repository, remote, 'main', 'main', []).catch(
      error => error
    )
    expect(isNativePushOutcomeUncertain(result)).toBe(true)
    const metadata: { retryAction?: string } = { retryAction: 'push' }
    suppressUnsafePushRetry(metadata, result, false)
    expect(metadata.retryAction).toBeUndefined()
    const priorFailure: { retryAction?: string } = { retryAction: 'push' }
    suppressUnsafePushRetry(
      priorFailure,
      new Error('failed before mutation'),
      false
    )
    expect(priorFailure.retryAction).toBe('push')
    suppressUnsafePushRetry(priorFailure, new Error('refresh failed'), true)
    expect(priorFailure.retryAction).toBeUndefined()
    const wrapped = new ErrorWithMetadata(result, { repository })
    const showPopup = vi.fn()
    const dispatcher = { showPopup } as unknown as Dispatcher
    expect(await pushNeedsPullHandler(wrapped, dispatcher)).toBe(wrapped)
    expect(await samlReauthRequired(wrapped, dispatcher)).toBe(wrapped)
    expect(showPopup).not.toHaveBeenCalled()
  })

  it('does not turn a completed push into an error when a progress observer throws during flush', async () => {
    mocks.push.mockImplementation(async (_path, _request, onProgress) => {
      onProgress({ stream: 'stderr', data: btoa('Writing objects: 50% (1/2)') })
    })
    let calls = 0
    const progress = () => {
      if (++calls > 1) throw new Error('observer failed')
    }
    await expect(
      push(repository, remote, 'main', null, [], undefined, progress)
    ).resolves.toBeUndefined()
    expect(calls).toBe(2)
  })

  it('delivers request-scoped output to a terminal subscriber without letting it fail a push', async () => {
    const chunks: string[] = []
    mocks.push.mockImplementation(
      async (_path, request, onProgress, checkDestinations) => {
        expect(request.report_progress).toBe(true)
        await checkDestinations(['https://example.invalid/owner/repo'])
        onProgress({ stream: 'stderr', data: btoa('remote: hello\n') })
      }
    )
    await push(repository, remote, 'main', null, [], {
      onTerminalOutputAvailable: subscribe => {
        subscribe(chunk => {
          chunks.push(String(chunk))
          throw new Error('observer failed')
        })
      },
    })
    expect(chunks).toEqual(['remote: hello\n'])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('replays bounded output to a late subscriber outside the repository operation', async () => {
    let subscribeLate:
      | ((listener: (chunk: string | Buffer | Buffer[]) => void) => unknown)
      | undefined
    const chunks: string[] = []
    mocks.push.mockImplementation(
      async (_path, _request, onProgress, checkDestinations) => {
        await checkDestinations(['https://example.invalid/owner/repo'])
        for (let i = 0; i < 4; i++)
          onProgress({
            stream: 'stderr',
            data: btoa(String(i).repeat(100_000)),
          })
        expect(subscribeLate).toBeDefined()
        subscribeLate!(chunk => {
          expect(mocks.detached).toBe(true)
          chunks.push(String(chunk))
        })
        onProgress({ stream: 'stderr', data: btoa('last') })
      }
    )
    await push(repository, remote, 'main', null, [], {
      onTerminalOutputAvailable: subscribe => {
        expect(mocks.detached).toBe(true)
        subscribeLate = subscribe
      },
    })
    const replay = chunks.join('')
    expect(replay.length).toBe(256 * 1024 + 4)
    expect(replay.endsWith(`${'3'.repeat(100_000)}last`)).toBe(true)
    expect(mocks.without).toHaveBeenCalled()
  })

  it('does not fall back if terminal observer setup throws a proxy-shaped error', async () => {
    mocks.transport = 'wry'
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invokeWithProgress: vi.fn() },
    })
    mocks.push.mockImplementation(
      async (_path, _request, _onProgress, checkDestinations) => {
        await checkDestinations(['https://example.invalid/owner/repo'])
      }
    )
    await push(repository, remote, 'main', null, [], {
      onTerminalOutputAvailable: () => {
        throw new mocks.PreviewUnsupported()
      },
    })
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'forwards request-owned pre-push progress and abort for %s',
    async transport => {
      mocks.transport = transport
      Object.defineProperty(window, '__desktop', {
        configurable: true,
        value: { invokeWithProgress: vi.fn() },
      })
      const progress = vi.fn(
        (event: { status: string; abort?: () => void }) => {
          expect(mocks.detached).toBe(true)
          if (event.status === 'started') event.abort?.()
        }
      )
      mocks.push.mockImplementation(
        async (
          _path,
          _request,
          _output,
          _check,
          onHook,
          signal: AbortSignal
        ) => {
          onHook({ hookName: 'pre-push', status: 'started' })
          expect(signal.aborted).toBe(true)
          onHook({ hookName: 'pre-push', status: 'failed' })
          return { exitCode: 1, errorKind: null }
        }
      )
      await expect(
        push(repository, remote, 'main', 'main', [], {
          onHookProgress: progress,
        })
      ).rejects.toThrow('Ignoring an individual failed hook is not supported')
      expect(progress.mock.calls.map(call => call[0].status)).toEqual([
        'started',
        'failed',
      ])
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )
})
