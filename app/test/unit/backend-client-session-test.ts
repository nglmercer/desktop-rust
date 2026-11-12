import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

describe('backend Copilot session cleanup', () => {
  afterEach(() => {
    Reflect.deleteProperty(window, '__desktop')
  })

  it('validates repository status context responses', async () => {
    const invoke = vi.fn(
      async (): Promise<unknown> => ({
        mergeHeadFound: false,
        squashMsgFound: true,
        isCherryPickingHeadFound: false,
        rebaseInternalState: {
          originalBranchTip: 'a',
          targetBranch: 'topic',
          baseBranchTip: 'b',
        },
      })
    )
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send: vi.fn(), on: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.repositoryStatusContext('/repo-a')
    ).resolves.toMatchObject({
      squashMsgFound: true,
      rebaseInternalState: { targetBranch: 'topic' },
    })
    invoke.mockResolvedValueOnce({
      mergeHeadFound: false,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: { targetBranch: 'topic' },
    })
    await expect(
      client.repositoryStatusContext('/repo-a')
    ).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates revision and merge history responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'commit output')
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send: vi.fn(), on: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.repositoryRevisionList('/repo-a', 'base..topic')
    ).resolves.toBe('commit output')
    invoke.mockResolvedValueOnce(false)
    await expect(
      client.repositoryRevisionList('/repo-a', 'base..topic')
    ).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(true)
    await expect(
      client.repositoryHasMergeCommits('/repo-a', 'HEAD')
    ).resolves.toBe(true)
    invoke.mockResolvedValueOnce('true')
    await expect(
      client.repositoryHasMergeCommits('/repo-a', 'HEAD')
    ).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates ahead and behind counts', async () => {
    const invoke = vi.fn(
      async (): Promise<unknown> => ({ ahead: 3, behind: 2 })
    )
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send: vi.fn(), on: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.repositoryAheadBehind('/repo-a', 'main...topic')
    ).resolves.toEqual({ ahead: 3, behind: 2 })
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.read',
      expect.objectContaining({
        repository_path: '/repo-a',
        method: 'ahead_behind',
        range: 'main...topic',
      })
    )
    for (const value of [
      { ahead: -1, behind: 1 },
      { ahead: 1.5, behind: 1 },
      { ahead: Number.MAX_SAFE_INTEGER + 1, behind: 1 },
      { ahead: 1, behind: '2' },
    ]) {
      invoke.mockResolvedValueOnce(value)
      await expect(
        client.repositoryAheadBehind('/repo-a', 'main...topic')
      ).rejects.toMatchObject({ code: 'invalid_response' })
    }
  })

  it('validates typed branch enumeration responses', async () => {
    const invoke = vi.fn(
      async (): Promise<unknown> => [
        {
          ref: 'refs/heads/main',
          name: 'main',
          upstream: null,
          upstream_ref: null,
          current: true,
          oid: 'abc',
        },
      ]
    )
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send: vi.fn(), on: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.repositoryBranches('/repo-a', ['refs/heads'])
    ).resolves.toMatchObject([{ name: 'main' }])
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.read',
      expect.objectContaining({
        repository_path: '/repo-a',
        method: 'branch_enumeration',
        prefixes: ['refs/heads'],
      })
    )
    invoke.mockResolvedValueOnce([
      {
        ref: 'refs/heads/main',
        name: 'main',
        upstream: null,
        upstream_ref: null,
        current: true,
        oid: 42,
      },
    ])
    await expect(
      client.repositoryBranches('/repo-a', [])
    ).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates merged and point-at branch responses', async () => {
    const invoke = vi.fn(
      async (): Promise<unknown> => [{ ref: 'refs/heads/topic', oid: 'abc' }]
    )
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send: vi.fn(), on: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.mergedRepositoryBranches('/repo-a', 'main')
    ).resolves.toEqual([{ ref: 'refs/heads/topic', oid: 'abc' }])
    invoke.mockResolvedValueOnce([{ ref: 'refs/heads/topic', oid: null }])
    await expect(
      client.mergedRepositoryBranches('/repo-a', 'main')
    ).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(null)
    await expect(
      client.branchesPointingAt('/repo-a', 'missing')
    ).resolves.toBeNull()
    invoke.mockResolvedValueOnce(['topic', 42])
    await expect(
      client.branchesPointingAt('/repo-a', 'HEAD')
    ).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('sends the session identifier to the host and returns its reply', async () => {
    const invoke = vi.fn(async () => ({ closed: true }))
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(client.copilotCloseSession('session-1')).resolves.toEqual({
      closed: true,
    })
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.copilot_close_session',
      {
        session_id: 'session-1',
      }
    )
  })

  it('requests read-only state for the explicitly selected repository', async () => {
    const invoke = vi.fn(async () => ({ root: '/repo-a' }))
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.readRepositoryState('/repo-a', 'snapshot')
    ).resolves.toEqual({ root: '/repo-a' })
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.read',
      expect.objectContaining({
        repository_path: '/repo-a',
        method: 'snapshot',
        request_id: expect.any(String),
        deadline_ms: 60000,
      })
    )
    expect(client.capabilities().cancel).toBe(true)
    await expect(
      client.request('other', {}, { signal: new AbortController().signal })
    ).rejects.toMatchObject({ code: 'unsupported' })
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it('passes the untracked mode through to the byte-preserving status operation', async () => {
    const records = [{ kind: 'entry', path: { bytes: [255] } }]
    const invoke = vi.fn(async () => records)
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.detailedRepositoryStatus('/repo-a', false)
    ).resolves.toEqual(records)
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.read',
      expect.objectContaining({
        repository_path: '/repo-a',
        method: 'status_detailed',
        include_untracked: false,
        request_id: expect.any(String),
        deadline_ms: 60000,
      })
    )
  })

  it('cancels the matching read and removes its abort listener after completion', async () => {
    let complete!: (value: unknown) => void
    const invoke = vi.fn(
      (
        _channel: string,
        _params: { request_id: string; deadline_ms: number }
      ) =>
        new Promise(resolve => {
          complete = resolve
        })
    )
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send, on: vi.fn() },
    })
    const client = new BackendClient()
    const controller = new AbortController()
    const result = client.detailedRepositoryStatus('/repo-a', true, {
      signal: controller.signal,
      deadlineMs: 5000,
    })
    const request = invoke.mock.calls[0][1]
    expect(request.deadline_ms).toBe(5000)
    controller.abort()
    await expect(result).rejects.toMatchObject({ code: 'cancelled' })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', {
      request_id: request.request_id,
    })
    complete([])

    const second = new AbortController()
    const done = client.detailedRepositoryStatus('/repo-a', true, {
      signal: second.signal,
    })
    complete([])
    await expect(done).resolves.toEqual([])
    second.abort()
    expect(send).toHaveBeenCalledTimes(1)
  })

  it('does not dispatch a pre-cancelled read and cancels on deadline', async () => {
    vi.useFakeTimers()
    try {
      const invoke = vi.fn(() => new Promise(() => {}))
      const send = vi.fn()
      Object.defineProperty(window, '__desktop', {
        configurable: true,
        value: { invoke, send, on: vi.fn() },
      })
      const client = new BackendClient()
      const controller = new AbortController()
      controller.abort()
      await expect(
        client.detailedRepositoryStatus('/repo-a', true, {
          signal: controller.signal,
        })
      ).rejects.toMatchObject({ code: 'cancelled' })
      expect(invoke).not.toHaveBeenCalled()
      const result = client.detailedRepositoryStatus('/repo-a', true, {
        deadlineMs: 10,
      })
      const rejected = expect(result).rejects.toMatchObject({
        code: 'ETIMEDOUT',
      })
      vi.advanceTimersByTime(10)
      await rejected
      expect(send).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it('does not claim cancellation when the host only has protocol fallback', async () => {
    const invoke = vi.fn(async () => [])
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: {
        invoke,
        send: vi.fn(),
        on: vi.fn(),
        supportsCancellation: () => false,
      },
    })
    const client = new BackendClient()
    expect(client.capabilities().cancel).toBe(false)
    await expect(
      client.detailedRepositoryStatus('/repo-a', true, {
        signal: new AbortController().signal,
      })
    ).rejects.toMatchObject({ code: 'unsupported' })
    expect(invoke).not.toHaveBeenCalled()
  })
})
