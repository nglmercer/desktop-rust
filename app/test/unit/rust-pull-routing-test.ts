import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  hooksEnabled: false,
  detached: false,
  pull: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
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
  PreviewUnsupported: class extends Error {},
  GitError: class extends Error {
    public readonly result: unknown
    public constructor(result: { gitErrorDescription: string }) {
      super(result.gitErrorDescription)
      this.result = result
    }
  },
  convert: vi.fn(() => new Error('classified fetch failure')),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    pullRepositoryRemote: mocks.pull,
  }),
  FetchPreviewUnsupported: mocks.PreviewUnsupported,
}))
vi.mock('../../src/lib/git/core', () => ({
  git: mocks.git,
  gitRebaseArguments: () => ['-c', 'rebase.backend=merge'],
  getDescriptionForError: () => 'conflicts',
  GitError: mocks.GitError,
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: mocks.environment,
  envForProxy: mocks.proxy,
}))
vi.mock('../../src/lib/git/config', () => ({
  getConfigValue: async () => null,
}))
vi.mock('../../src/lib/hooks/config', () => ({
  getHooksEnvEnabled: () => mocks.hooksEnabled,
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
  withoutRepositoryGitAccess: mocks.without,
}))
vi.mock('../../src/lib/git/fetch-error', () => ({
  fetchFailureToGitError: mocks.convert,
}))

import { pull } from '../../src/lib/git/pull'

const repository = { path: '/repo', gitHubRepository: null } as Repository
const remote = { name: 'origin', url: 'https://example.invalid/owner/repo' }

describe('Rust pull routing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    for (const key of [
      'http_proxy',
      'https_proxy',
      'HTTPS_PROXY',
      'all_proxy',
      'ALL_PROXY',
    ]) {
      vi.stubEnv(key, undefined)
    }
    mocks.transport = 'ws'
    mocks.hooksEnabled = false
    mocks.detached = false
    mocks.pull.mockReset().mockResolvedValue(undefined)
    mocks.git.mockReset().mockResolvedValue(undefined)
    mocks.proxy.mockReset().mockResolvedValue(undefined)
  })
  afterEach(() => {
    vi.unstubAllEnvs()
    Reflect.deleteProperty(window, '__desktop')
  })

  it.each(['ws', 'wry'])(
    'runs one reviewed pull through %s when hook environment is disabled',
    async transport => {
      mocks.transport = transport
      Object.defineProperty(window, '__desktop', {
        configurable: true,
        value: { invokeWithProgress: vi.fn() },
      })
      const hookFailure = vi.fn()
      const progress = vi.fn()
      await pull(repository, remote, {
        onHookFailure: hookFailure,
        progressCallback: progress,
      })
      expect(mocks.pull).toHaveBeenCalledTimes(1)
      expect(mocks.pull).toHaveBeenCalledWith(
        '/repo',
        {
          remote: 'origin',
          expected_url: remote.url,
          no_verify: false,
          report_progress: true,
          require_hook_callbacks: false,
        },
        expect.any(Function)
      )
      expect(mocks.git).not.toHaveBeenCalled()
      expect(hookFailure).not.toHaveBeenCalled()
      expect(progress).toHaveBeenCalledWith(
        expect.objectContaining({ kind: 'pull', value: 0 })
      )
    }
  )

  it('uses the prior Wry route before mutation when the hook environment is enabled', async () => {
    mocks.transport = 'wry'
    mocks.hooksEnabled = true
    await pull(repository, remote, { onHookFailure: vi.fn() })
    expect(mocks.pull).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledTimes(1)
    expect(mocks.git.mock.calls[0][0]).toEqual([
      '-c',
      'rebase.backend=merge',
      'pull',
      '--ff',
      '--recurse-submodules',
      '--',
      'origin',
    ])
  })

  it('reports the unsupported hook environment on WebSocket before mutation', async () => {
    mocks.hooksEnabled = true
    await expect(pull(repository, remote)).rejects.toBeInstanceOf(
      mocks.PreviewUnsupported
    )
    expect(mocks.pull).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('recognizes the enabled default in development and beta builds', async () => {
    const previousDev = __DEV__
    const previousChannel = __RELEASE_CHANNEL__
    const previousPreview = process.env.GITHUB_DESKTOP_PREVIEW_FEATURES
    const previousSetting = localStorage.getItem('git-hooks-env-enabled')
    try {
      delete process.env.GITHUB_DESKTOP_PREVIEW_FEATURES
      localStorage.removeItem('git-hooks-env-enabled')
      for (const [dev, channel] of [
        [true, 'production'],
        [false, 'beta'],
      ] as const) {
        Object.assign(globalThis, {
          __DEV__: dev,
          __RELEASE_CHANNEL__: channel,
        })
        vi.resetModules()
        const config = await vi.importActual<
          typeof import('../../src/lib/hooks/config')
        >('../../src/lib/hooks/config')
        expect(config.getHooksEnvEnabled()).toBe(true)
      }
    } finally {
      Object.assign(globalThis, {
        __DEV__: previousDev,
        __RELEASE_CHANNEL__: previousChannel,
      })
      if (previousPreview === undefined) {
        delete process.env.GITHUB_DESKTOP_PREVIEW_FEATURES
      } else {
        process.env.GITHUB_DESKTOP_PREVIEW_FEATURES = previousPreview
      }
      if (previousSetting === null) {
        localStorage.removeItem('git-hooks-env-enabled')
      } else {
        localStorage.setItem('git-hooks-env-enabled', previousSetting)
      }
      vi.resetModules()
    }
  })

  it('falls back only for a preflight refusal, never after an uncertain result', async () => {
    mocks.transport = 'wry'
    mocks.pull.mockRejectedValueOnce(
      new mocks.PreviewUnsupported('unsupported')
    )
    await pull(repository, remote)
    expect(mocks.git).toHaveBeenCalledTimes(1)
    mocks.git.mockClear()
    mocks.pull.mockRejectedValueOnce(new Error('pull outcome uncertain'))
    await expect(pull(repository, remote)).rejects.toThrow(
      'pull outcome uncertain'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('maps conflicts and network failures without retrying the Git operation', async () => {
    mocks.pull.mockResolvedValueOnce({
      exitCode: 1,
      errorKind: 'merge_conflicts',
    })
    await expect(pull(repository, remote)).rejects.toBeInstanceOf(
      mocks.GitError
    )
    mocks.pull.mockResolvedValueOnce({
      exitCode: 128,
      errorKind: 'https_authentication_failed',
    })
    await expect(pull(repository, remote)).rejects.toThrow(
      'classified fetch failure'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('detaches progress observers from the operation and ignores observer exceptions', async () => {
    mocks.pull.mockImplementationOnce(async (_path, _request, onProgress) => {
      onProgress({
        stream: 'stderr',
        data: btoa('Receiving objects: 50% (1/2)\r'),
      })
    })
    const seen: boolean[] = []
    await pull(repository, remote, {
      progressCallback: () => {
        seen.push(mocks.detached)
        throw new Error('observer failed')
      },
    })
    expect(seen.length).toBeGreaterThanOrEqual(2)
    expect(seen.every(Boolean)).toBe(true)
    expect(mocks.detached).toBe(false)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
