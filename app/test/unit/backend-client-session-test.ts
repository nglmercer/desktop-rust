import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

describe('backend Copilot session cleanup', () => {
  afterEach(() => {
    Reflect.deleteProperty(window, '__desktop')
  })

  it('cancels a Rust commit without treating its outcome as retryable', async () => {
    const invoke = vi.fn(() => new Promise<unknown>(() => {}))
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send, on: vi.fn() } })
    const client = new BackendClient()
    const controller = new AbortController()
    const id = '12345678-1234-4234-8234-123456789abc'
    const pending = client.publishSelectedCommit('/repo', id, 'message', { signal: controller.signal })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'commit_checkpoint', operation: expect.objectContaining({ method: 'publish_selected_commit', checkpoint_id: id }) }), request_id: expect.any(String) }))
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect commit checkpoints') })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', { request_id: expect.any(String) })
  })

  it('cancels checkpoint preparation and directs the caller to inspect history', async () => {
    const invoke = vi.fn(() => new Promise<unknown>(() => {}))
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send, on: vi.fn() } })
    const controller = new AbortController()
    const pending = new BackendClient().prepareSelectedCommit('/repo', { paths: ['file'] }, 'a'.repeat(64), { signal: controller.signal, deadlineMs: 20_000 })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ deadline_ms: 20_000 }))
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect commit checkpoints') })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', { request_id: expect.any(String) })
  })

  it('delivers only matching commit hook progress and removes the listener', async () => {
    let finish: (value: unknown) => void = () => {}
    const invoke = vi.fn(() => new Promise<unknown>(resolve => { finish = resolve }))
    const on = vi.fn()
    const off = vi.fn()
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on, off } })
    const id = '12345678-1234-4234-8234-123456789abc'
    const progress = vi.fn()
    const output = vi.fn()
    const pending = new BackendClient().publishSelectedCommit('/repo', id, 'message', { onHookProgress: progress, onOutput: output })
    const listener = on.mock.calls[0][1]
    listener({}, { checkpoint_id: 'other', hook_name: 'pre-commit', status: 'started' })
    listener({}, { checkpoint_id: id, hook_name: 'unknown', status: 'started' })
    listener({}, { checkpoint_id: id, hook_name: 'pre-commit', status: 'started' })
    expect(progress).toHaveBeenCalledOnce()
    expect(progress).toHaveBeenCalledWith({ hookName: 'pre-commit', status: 'started' })
    const outputListener = on.mock.calls[1][1]
    outputListener({}, { checkpoint_id: 'other', stream: 'stderr', data: 'aGk=' })
    outputListener({}, { checkpoint_id: id, stream: 'stderr', data: 'invalid!' })
    outputListener({}, { checkpoint_id: id, stream: 'stderr', data: 'aGk=' })
    expect(output).toHaveBeenCalledOnce()
    expect(output).toHaveBeenCalledWith({ stream: 'stderr', bytes: new Uint8Array([104, 105]) })
    finish({ id, state: 'complete', oid: 'b'.repeat(40), stdout: '', stderr: '', output_truncated: false })
    await pending
    expect(off).toHaveBeenCalledWith('commit_hook_progress', listener)
    expect(off).toHaveBeenCalledWith('commit_output', outputListener)
  })

  it.each([
    { type: 'set', key: 'user.name', value: 'Alice' },
    { type: 'set', key: 'init.defaultbranch', value: 'trunk' },
    { type: 'set_repository', repository_path: '/repo', key: 'user.name', value: 'Alice' },
    { type: 'unset_repository', repository_path: '/repo', key: 'user.name' },
  ])('treats interrupted $type identity writes as uncertain', async operation => {
    const invoke = vi.fn(() => new Promise<unknown>(() => {}))
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, send, on: vi.fn() },
    })
    const client = new BackendClient()
    const controller = new AbortController()
    const pending = client.request('git_info', { operation }, { signal: controller.signal })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation, request_id: expect.any(String), deadline_ms: 60000 }))
    controller.abort()
    await expect(pending).rejects.toMatchObject({
      code: 'cancelled',
      retryable: false,
      message: expect.stringContaining('inspect Git settings'),
    })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', { request_id: expect.any(String) })
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

  it('finishes an existing merge through the native host without retrying interruptions', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '[main abc1234] Merge\n', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.finishRepositoryMergeCommit('/repo')).resolves.toContain('abc1234')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'finish_merge_commit', repository_path: '/repo', operation: { method: 'renderer_finish_merge_commit' } } }))
    invoke.mockRejectedValueOnce({ code: 'cancelled', message: 'interrupted', retryable: true })
    await expect(client.finishRepositoryMergeCommit('/repo')).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect HEAD') })
    expect(invoke).toHaveBeenCalledTimes(2)
  })

  it('binds squash abort to the reviewed branch and tip', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const head = 'a'.repeat(40)
    await client.abortRepositorySquashMerge('/repo', head, 'refs/heads/main')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'abort_git_operation', repository_path: '/repo', operation: { method: 'renderer_abort_squash', expected_head: head, expected_branch: 'refs/heads/main' } } }))
    await expect(client.abortRepositorySquashMerge('/repo', 'bad', 'refs/heads/main')).rejects.toMatchObject({ code: 'invalid_request', retryable: false })
    invoke.mockRejectedValueOnce({ code: 'cancelled', message: 'interrupted', retryable: true })
    await expect(client.abortRepositorySquashMerge('/repo', head, 'refs/heads/main')).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('working files') })
    expect(invoke).toHaveBeenCalledTimes(2)
  })

  it('uses a restricted native abort and does not retry uncertain results', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.abortRepositoryGitOperation('/repo', 'merge')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'abort_git_operation', repository_path: '/repo', operation: { method: 'recover', kind: 'merge', action: 'abort' } } }))
    invoke.mockRejectedValueOnce({ code: 'cancelled', message: 'interrupted', retryable: true })
    await expect(client.abortRepositoryGitOperation('/repo', 'rebase')).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect the repository') })
    invoke.mockResolvedValueOnce({ code: 1, stdout: '', stderr: 'abort failed' })
    await expect(client.abortRepositoryGitOperation('/repo', 'cherry_pick')).rejects.toMatchObject({ code: 'git_error', retryable: false, message: expect.stringContaining('abort failed') })
    expect(invoke).toHaveBeenCalledTimes(3)
  })

  it('reads and validates a scoped native rebase snapshot', async () => {
    const head = 'a'.repeat(40)
    const invoke = vi.fn(async (): Promise<unknown> => ({ head, progress: { original: head, branch: 'refs/heads/main', onto: head, current: '1', total: '2' } }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryRebaseState('/repo')).resolves.toMatchObject({ head, progress: { current: '1', total: '2' } })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'renderer_rebase_state' }))
    invoke.mockResolvedValueOnce({ head, progress: { current: 2 } })
    await expect(client.repositoryRebaseState('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('reads and validates a scoped native cherry-pick snapshot', async () => {
    const head = 'a'.repeat(40)
    const invoke = vi.fn(async (): Promise<unknown> => ({ head, sequence: { head, abort_safety: head, todo: 'pick abc1234 subject\n' } }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryCherryPickState('/repo')).resolves.toEqual({ head, sequence: { head, abortSafety: head, todo: 'pick abc1234 subject\n' } })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'renderer_cherry_pick_state' }))
    invoke.mockResolvedValueOnce({ head: 'invalid', sequence: null })
    await expect(client.repositoryCherryPickState('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('reads Git version through the native scoped service and validates the response', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => '2.44.0')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.gitVersion()).resolves.toBe('2.44.0')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'version' } }))
    invoke.mockResolvedValueOnce({ version: '2.44.0' })
    await expect(client.gitVersion()).rejects.toMatchObject({ code: 'invalid_response' })
  })
  it('validates the native global configuration path', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => '/home/test/config with spaces')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.globalGitConfigPath()).resolves.toBe('/home/test/config with spaces')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'global_config_path' } }))
    for (const value of ['', '/bad\0path', { path: '/tmp/config' }]) {
      invoke.mockResolvedValueOnce(value)
      await expect(client.globalGitConfigPath()).rejects.toMatchObject({ code: 'invalid_response' })
    }
  })
  it('treats interrupted global configuration preparation as uncertain', async () => {
    const invoke = vi.fn(() => new Promise<unknown>(() => {}))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const controller = new AbortController()
    const pending = new BackendClient().request('git_info', { operation: { type: 'global_config_path' } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('check the configuration file') })
  })

  it('validates scoped Git LFS status and file tracking responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ available: true, using_lfs: true }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryUsesLfs('/repo')).resolves.toBe(true)
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'lfs_status' }))
    invoke.mockResolvedValueOnce({ available: false, using_lfs: null })
    await expect(client.repositoryUsesLfs('/repo')).resolves.toBe(false)
    invoke.mockResolvedValueOnce({ available: true, using_lfs: null })
    await expect(client.repositoryUsesLfs('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce([true, false, true])
    await expect(client.repositoryLfsFileTracking('/repo', ['a', 'b', 'a'])).resolves.toEqual([true, false, true])
    expect(invoke).toHaveBeenLastCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'lfs_file_tracking', paths: ['a', 'b', 'a'] }))
    for (const response of [[true], [true, false, 'true']]) {
      invoke.mockResolvedValueOnce(response)
      await expect(client.repositoryLfsFileTracking('/repo', ['a', 'b', 'a'])).rejects.toMatchObject({ code: 'invalid_response' })
    }
  })

  it('routes LFS hook installation to the requested Wry repository and reports failure', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: 'Git LFS initialized', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.installRepositoryLfsHooks('/repo', false)
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'lfs_install_repository', repository_path: '/repo', force: false } }))
    invoke.mockResolvedValueOnce({ code: 2, stdout: '', stderr: 'configuration error', error_kind: 'configuration' })
    await expect(client.installRepositoryLfsHooks('/repo', false)).rejects.toMatchObject({ code: 'lfs_install_failed', retryable: false })
    invoke.mockResolvedValueOnce('Git LFS initialized')
    await expect(client.installGlobalLfsFilters(false)).rejects.toMatchObject({ code: 'invalid_response', retryable: false })
    await expect(client.installGlobalLfsFilters(false)).resolves.toBeUndefined()
  })

  it('reports interrupted Wry LFS setup as uncertain', async () => {
    const invoke = vi.fn(() => new Promise<unknown>(() => {}))
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send, on: vi.fn() } })
    const client = new BackendClient()
    const controller = new AbortController()
    const pending = client.request('git_info', { operation: { type: 'lfs_install_repository', repository_path: '/repo', force: true } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect Git settings and hooks') })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', { request_id: expect.any(String) })
  })

  it('routes repository Git identity reads and rejects malformed responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'Local Author')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryGitIdentityValue('/repo-a', 'user.name', true)).resolves.toBe('Local Author')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo-a', method: 'git_identity_value', key: 'user.name', only_local: true }))
    invoke.mockResolvedValueOnce({ value: 'unexpected' })
    await expect(client.repositoryGitIdentityValue('/repo-a', 'user.email', false)).rejects.toMatchObject({ code: 'invalid_response' })
  })
  it('reads the repository description through the scoped native service', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'Description\n')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryGitDescription('/repo')).resolves.toBe('Description\n')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'git_description' }))
    invoke.mockResolvedValueOnce({ description: 'unexpected' })
    await expect(client.repositoryGitDescription('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })
  it('accepts only credential-free submodule display URLs', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'https://github.com/owner/repo')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySubmoduleDisplayUrl('/repo', 'libs/nested')).resolves.toBe('https://github.com/owner/repo')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'submodule_display_url', submodule_path: 'libs/nested' }))
    invoke.mockResolvedValueOnce(null)
    await expect(client.repositorySubmoduleDisplayUrl('/repo', 'missing')).resolves.toBeNull()
    for (const value of ['https://user:secret@github.com/a/b', 'https://github.com/a/b?token=secret', 'file:///tmp/a']) {
      invoke.mockResolvedValueOnce(value)
      await expect(client.repositorySubmoduleDisplayUrl('/repo', 'libs/nested')).rejects.toMatchObject({ code: 'invalid_response' })
    }
  })
  it('validates description writes and reports interrupted native writes as uncertain', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.setRepositoryGitDescription('/repo', 'text')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'set_description', repository_path: '/repo', description: 'text' } }))
    invoke.mockResolvedValueOnce({ ok: true })
    await expect(client.setRepositoryGitDescription('/repo', 'text')).rejects.toMatchObject({ code: 'invalid_response', retryable: false })
    invoke.mockImplementationOnce(() => new Promise<unknown>(() => {}))
    const controller = new AbortController()
    const pending = client.request('git_info', { operation: { type: 'set_description', repository_path: '/repo', description: 'text' } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect the repository description') })
  })
  it('initializes a specific repository and treats interruption as uncertain', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.initializeDesktopRepository('/new/repo', 'trunk')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'initialize_repository', repository_path: '/new/repo', default_branch: 'trunk' } }))
    invoke.mockImplementationOnce(() => new Promise<unknown>(() => {}))
    const controller = new AbortController()
    const pending = client.request('git_info', { operation: { type: 'initialize_repository', repository_path: '/new/repo', default_branch: 'trunk' } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect the destination') })
  })
  it('validates initial commit results and treats interruption as uncertain', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.createInitialCommit('/new/repo', ['README.md'])
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'initial_commit', repository_path: '/new/repo', paths: ['README.md'] } }))
    invoke.mockResolvedValueOnce({ code: 1, stderr: 'hook failed' })
    await expect(client.createInitialCommit('/new/repo', ['README.md'])).rejects.toMatchObject({ code: 'initial_commit_failed', retryable: false, message: expect.stringContaining('inspect staged files') })
    invoke.mockImplementationOnce(() => new Promise<unknown>(() => {}))
    const controller = new AbortController()
    const pending = client.request('git_info', { operation: { type: 'initial_commit', repository_path: '/new/repo', paths: ['README.md'] } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect the repository and index') })
  })
  it('does not mark a native cancellation response as retryable for a write', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => { throw { code: 'cancelled', message: 'inspect the destination' } })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.initializeDesktopRepository('/repo', 'main')).rejects.toMatchObject({ code: 'cancelled', retryable: false })
    await expect(client.repositoryGitDescription('/repo')).rejects.toMatchObject({ code: 'cancelled', retryable: true })
  })
  it('validates scoped repository Git setting reads', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'true')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryGitReadSetting('/repo', 'commit.gpgsign', true, true)).resolves.toBe('true')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'git_read_setting', key: 'commit.gpgsign', only_local: true, canonical_bool: true }))
    invoke.mockResolvedValueOnce('yes')
    await expect(client.repositoryGitReadSetting('/repo', 'commit.gpgsign', false, true)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates the native remote discovery response', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => [{ name: 'origin', url: 'https://example.test/repo' }])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySafeFetchRemotes('/repo')).resolves.toEqual([{ name: 'origin', url: 'https://example.test/repo' }])
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'safe_fetch_remotes' }))
    invoke.mockResolvedValueOnce(null)
    await expect(client.repositorySafeFetchRemotes('/repo')).resolves.toBeNull()
    invoke.mockResolvedValueOnce([{ name: 'origin', url: 42 }])
    await expect(client.repositorySafeFetchRemotes('/repo')).rejects.toMatchObject({ code: 'invalid_response', retryable: false })
  })

  it('validates named native remote URL responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ kind: 'resolved', url: 'https://example.test/repo' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySafeRemoteUrl('/repo', 'origin')).resolves.toEqual({ kind: 'resolved', url: 'https://example.test/repo' })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'safe_remote_url', remote_name: 'origin' }))
    invoke.mockResolvedValueOnce({ kind: 'missing' }).mockResolvedValueOnce({ kind: 'unsupported' }).mockResolvedValueOnce({ kind: 'resolved', url: 42 })
    await expect(client.repositorySafeRemoteUrl('/repo', 'none')).resolves.toEqual({ kind: 'missing' })
    await expect(client.repositorySafeRemoteUrl('/repo', 'private')).resolves.toEqual({ kind: 'unsupported' })
    await expect(client.repositorySafeRemoteUrl('/repo', 'broken')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates native symbolic ref responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'refs/heads/main')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySymbolicRef('/repo', 'HEAD')).resolves.toBe('refs/heads/main')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'symbolic_ref', reference: 'HEAD' }))
    invoke.mockResolvedValueOnce(null).mockResolvedValueOnce({ ref: 'unexpected' })
    await expect(client.repositorySymbolicRef('/repo', 'HEAD')).resolves.toBeNull()
    await expect(client.repositorySymbolicRef('/repo', 'HEAD')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates and does not replay native remote changes', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ kind: 'updated' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const action = { type: 'add' as const, name: 'origin', url: 'https://example.test/repo' }
    await expect(client.manageRepositoryRemote('/repo', action)).resolves.toBe('updated')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'manage_remote', repository_path: '/repo', action } }))
    invoke.mockResolvedValueOnce({ kind: 'unsupported' }).mockResolvedValueOnce({ kind: 'other' })
    await expect(client.manageRepositoryRemote('/repo', action)).resolves.toBe('unsupported')
    await expect(client.manageRepositoryRemote('/repo', action)).rejects.toMatchObject({ code: 'invalid_response', retryable: false })
    invoke.mockImplementationOnce(() => new Promise<unknown>(() => {}))
    const controller = new AbortController()
    const pending = client.request('git_info', { operation: { type: 'manage_remote', repository_path: '/repo', action } }, { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect Git configuration') })
  })

  it('routes repository identity writes and removals through the scoped Wry operation', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ scope: 'local', values: {} }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.setRepositoryGitIdentityValue('/repo-a', 'user.name', 'Alice')
    await client.setRepositoryGitIdentityValue('/repo-a', 'user.email', null)
    expect(invoke).toHaveBeenNthCalledWith(1, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'set_repository', repository_path: '/repo-a', key: 'user.name', value: 'Alice' } }))
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'unset_repository', repository_path: '/repo-a', key: 'user.email' } }))
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

  it('rejects unrepresentable or malformed staged paths before discard', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => [
      { status: 'A', path: { bytes: [97, 10, 98] } },
    ])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryIndexChanges('/repo-a')).resolves.toEqual([{ status: 'A', path: 'a\nb' }])
    invoke.mockResolvedValueOnce([{ status: 'M', path: { bytes: [0xef, 0xbb, 0xbf, 97] } }])
    await expect(client.repositoryIndexChanges('/repo-a')).resolves.toEqual([{ status: 'M', path: '\ufeffa' }])
    invoke.mockResolvedValueOnce([{ status: 'A', path: { bytes: [255] } }])
    await expect(client.repositoryIndexChanges('/repo-a')).rejects.toMatchObject({ code: 'unsupported_path' })
    invoke.mockResolvedValueOnce([{ status: 'R', path: 'old' }])
    await expect(client.repositoryIndexChanges('/repo-a')).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce([{ status: 'A', path: 'same' }, { status: 'D', path: 'same' }])
    await expect(client.repositoryIndexChanges('/repo-a')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates blob preview lengths and responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'AA==')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryBlobPrefix('/repo', 'HEAD', 'file', 1)).resolves.toBe('AA==')
    invoke.mockResolvedValueOnce('AAAA')
    await expect(client.repositoryBlobPrefix('/repo', 'HEAD', 'file', 1)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce('bad!')
    await expect(client.repositoryBlobPrefix('/repo', 'HEAD', 'file', 3)).rejects.toMatchObject({ code: 'invalid_response' })
    await expect(client.repositoryBlobPrefix('/repo', 'HEAD', 'file', -1)).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates blob chunks and pins object identity', async () => {
    const oid = 'a'.repeat(40)
    const invoke = vi.fn(async (): Promise<unknown> => ({ oid, size: 2, data: 'AAE=' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryBlobChunk('/repo', 'HEAD', 'file', 0, 2, null)).resolves.toEqual({ oid, size: 2, data: 'AAE=' })
    invoke.mockResolvedValueOnce({ oid, size: 2, data: 'AA==' })
    await expect(client.repositoryBlobChunk('/repo', 'HEAD', 'file', 0, 2, oid)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ oid: 'b'.repeat(40), size: 2, data: 'AAE=' })
    await expect(client.repositoryBlobChunk('/repo', 'HEAD', 'file', 0, 2, oid)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(null)
    await expect(client.repositoryBlobChunk('/repo', 'HEAD', 'file', 1, 1, oid)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('reviews a ref-only branch creation and does not retry an uncertain write', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ basis: 'a'.repeat(64) }))
    invoke.mockResolvedValueOnce({ basis: 'a'.repeat(64) }).mockResolvedValueOnce({ code: 0, created: true, checked_out: false, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.createRepositoryBranch('/repo', 'feature', 'origin/main', true)
    expect(invoke).toHaveBeenNthCalledWith(1, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_creation', repository_path: '/repo', operation: { method: 'create_branch_preview', name: 'feature', start_point: 'origin/main', no_track: true, checkout: false } } }))
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_creation', repository_path: '/repo', operation: { method: 'create_branch', name: 'feature', start_point: 'origin/main', no_track: true, checkout: false, basis: 'a'.repeat(64) } } }))
    invoke.mockResolvedValueOnce({ basis: 'a'.repeat(64) }).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.createRepositoryBranch('/repo', 'other', null, false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect the branch') })
    expect(invoke).toHaveBeenCalledTimes(4)
  })

  it('binds branch deletion to the observed tip and refuses stale or current branches', async () => {
    const oid = 'b'.repeat(40)
    const preview = { basis: 'a'.repeat(64), branch: { ref: 'refs/heads/feature', oid }, current: false, checkout: null, in_use_worktree: null }
    const id = '12345678-1234-4234-8234-123456789abc'
    const invoke = vi.fn(async (): Promise<unknown> => preview)
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, receipt_id: id, backup_ref: `refs/desktop/deleted-branches/${id}` })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.deleteRepositoryBranch('/repo', 'feature', oid)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_deletion', repository_path: '/repo', operation: { method: 'delete_branch', name: 'refs/heads/feature', to_checkout: null, upstream: null, basis: 'a'.repeat(64), ref_only: true, force: true } } }))
    invoke.mockResolvedValueOnce(preview)
    await expect(client.deleteRepositoryBranch('/repo', 'feature', 'c'.repeat(40))).rejects.toMatchObject({ code: 'branch_changed' })
    invoke.mockResolvedValueOnce({ ...preview, current: true })
    await expect(client.deleteRepositoryBranch('/repo', 'feature', oid)).rejects.toMatchObject({ code: 'branch_changed' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.deleteRepositoryBranch('/repo', 'feature', oid)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('deletion recovery history') })
    expect(invoke).toHaveBeenCalledTimes(6)
  })

  it('pins a branch rename to its observed tip and reports uncertain outcomes', async () => {
    const oid = 'b'.repeat(40)
    const preview = { basis: 'a'.repeat(64), name: 'feature', new_name: 'renamed', oid }
    const receipt = { id: '12345678-1234-4234-8234-123456789abc', name: 'feature', new_name: 'renamed', oid, phase: 'complete' }
    const invoke = vi.fn(async (): Promise<unknown> => preview)
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, receipt })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.renameRepositoryBranch('/repo', 'feature', 'renamed', oid)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_rename', repository_path: '/repo', operation: { method: 'rename_branch', name: 'refs/heads/feature', new_name: 'renamed', basis: 'a'.repeat(64) } } }))
    invoke.mockResolvedValueOnce(preview)
    await expect(client.renameRepositoryBranch('/repo', 'feature', 'renamed', 'c'.repeat(40))).rejects.toMatchObject({ code: 'branch_changed' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.renameRepositoryBranch('/repo', 'feature', 'renamed', oid)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('saved changes') })
    invoke.mockResolvedValueOnce({ ...preview, new_name: 'feature' }).mockResolvedValueOnce({ code: 0, unchanged: true })
    await expect(client.renameRepositoryBranch('/repo', 'feature', 'feature', oid)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenCalledTimes(7)
  })

  it('saves, restores and drops only reviewed Desktop stashes', async () => {
    const oid = 'b'.repeat(40)
    const listing = { basis: 'a'.repeat(64), has_more: false, entries: [{ index: 0, oid, branch: 'feature' }] }
    const invoke = vi.fn(async (): Promise<unknown> => listing)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    invoke.mockResolvedValueOnce({ code: 0, created: true, stderr: '' })
    await expect(client.saveDesktopStash('/repo', 'feature')).resolves.toBe(true)
    invoke.mockResolvedValueOnce(listing).mockResolvedValueOnce({ code: 1, conflicted: true, retained: true, stderr: '' })
    await expect(client.mutateDesktopStash('/repo', oid, 'pop')).resolves.toBeUndefined()
    invoke.mockResolvedValueOnce(listing).mockResolvedValueOnce({ code: 0, retained: true, stderr: '' })
    await expect(client.mutateDesktopStash('/repo', oid, 'pop')).rejects.toMatchObject({ code: 'stash_retained', retryable: false })
    invoke.mockResolvedValueOnce({ ...listing, entries: [{ index: 0, oid, branch: null }] })
    await client.mutateDesktopStash('/repo', oid, 'drop')
    expect(invoke).toHaveBeenCalledTimes(6)
    invoke.mockResolvedValueOnce(listing).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.mutateDesktopStash('/repo', oid, 'drop')).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('saved changes') })
  })

  it('validates renderer stash file listings', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ':100644 100644\0file\0')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const oid = 'a'.repeat(40)
    await expect(client.repositoryStashChanges('/repo', oid)).resolves.toBe(':100644 100644\0file\0')
    await expect(client.repositoryStashChanges('/repo', 'HEAD')).rejects.toMatchObject({ code: 'invalid_request' })
    invoke.mockResolvedValueOnce({})
    await expect(client.repositoryStashChanges('/repo', oid)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('routes stash file listings through the Rust renderer operation', async () => {
    vi.resetModules()
    const oid = 'a'.repeat(40)
    const invoke = vi.fn(async (): Promise<unknown> => 'raw stash output')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const legacyGit = vi.fn()
    const parse = vi.fn(() => ({ files: ['parsed'] }))
    vi.doMock('../../src/lib/git/core', () => ({ git: legacyGit }))
    vi.doMock('../../src/lib/git/log', () => ({ parseRawLogWithNumstat: parse }))
    vi.doMock('../../src/lib/git/update-index', () => ({ stageFiles: vi.fn() }))
    vi.doMock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: (_path: string, _kind: string, run: () => unknown) => run() }))
    const { getStashedFiles } = await import('../../src/lib/git/stash')
    await expect(getStashedFiles({ path: '/repo' } as any, oid)).resolves.toEqual(['parsed'])
    expect(parse).toHaveBeenCalledWith('raw stash output', oid, `${oid}^`)
    expect(legacyGit).not.toHaveBeenCalled()
  })

  it('reviews a pinned branch checkout and never retries an uncertain switch', async () => {
    const oid = 'b'.repeat(40)
    const preview = { basis: 'a'.repeat(64), dirty: true, target: { reference: 'refs/heads/feature', oid }, source_branch: 'main', source_head: oid, source_stash: null, occupied_worktree: null }
    const invoke = vi.fn(async (): Promise<unknown> => preview)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.reviewBranchCheckout('/repo', 'refs/heads/feature', oid)).resolves.toEqual({ basis: 'a'.repeat(64), dirty: true, sourceBranch: 'main', sourceHead: oid, sourceStash: null })
    invoke.mockResolvedValueOnce(preview)
    await expect(client.reviewBranchCheckout('/repo', 'refs/heads/feature', 'c'.repeat(40))).rejects.toMatchObject({ code: 'branch_changed' })
    invoke.mockResolvedValueOnce({ code: 0, checked_out: true, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    await client.checkoutReviewedBranch('/repo', 'refs/heads/feature', 'a'.repeat(64), 'MoveToNewBranch', false)
    invoke.mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.checkoutReviewedBranch('/repo', 'refs/heads/feature', 'a'.repeat(64), 'MoveToNewBranch', false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('checkout history') })
    expect(invoke).toHaveBeenCalledTimes(4)
  })

  it('uses reviewed direct checkout without silently stashing local changes', async () => {
    const oid = 'b'.repeat(40)
    const preview = { basis: 'a'.repeat(64), dirty: true, target: { reference: 'refs/heads/feature', oid }, source_branch: 'main', source_head: oid, source_stash: null, occupied_worktree: null }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, checked_out: true, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    await new BackendClient().checkoutDirectBranch('/repo', 'refs/heads/feature', oid, true)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_checkout', repository_path: '/repo', operation: { method: 'renderer_checkout_direct', branch: 'refs/heads/feature', basis: 'a'.repeat(64), allow_file_protocol: true } } }))
  })

  it('validates detached commit reviews and reports interrupted checkout', async () => {
    const oid = 'b'.repeat(40)
    const preview = { basis: 'a'.repeat(64), target: { oid, detached: true } }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, checked_out: true, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.checkoutDetachedCommit('/repo', oid, true)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'branch_checkout', repository_path: '/repo', operation: { method: 'renderer_detach', commit: oid, basis: 'a'.repeat(64), allow_file_protocol: true } } }))
    invoke.mockResolvedValueOnce({ ...preview, target: { oid: 'c'.repeat(40), detached: true } })
    await expect(client.checkoutDetachedCommit('/repo', oid, false)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.checkoutDetachedCommit('/repo', oid, false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('checkout history') })
  })

  it('reviews remote fetches and does not replay interrupted mutations', async () => {
    const request = { remote: '--Tópico', expected_url: 'file:///repo', recurse_submodules: false, is_background_task: false, report_progress: false }
    const preview = { remote: request.remote, recurse_submodules: false, basis: 'c'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, error_kind: null })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'fetch_remote', repository_path: '/repo', operation: { method: 'renderer_fetch', request, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, remote: 'other' })
    await expect(client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected secret'))
    await expect(client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false)).rejects.toMatchObject({ retryable: false, message: expect.not.stringContaining('secret') })
    expect(invoke).toHaveBeenCalledTimes(5)
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 128, error_kind: 'https_authentication_failed' })
    await expect(client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false)).resolves.toEqual({ exitCode: 128, errorKind: 'https_authentication_failed' })
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 128, error_kind: 'unknown secret' })
    await expect(client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false)).rejects.toMatchObject({ code: 'invalid_response', message: expect.not.stringContaining('secret') })
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 0, error_kind: null })
    await client.fetchRepositoryRemote('/repo', request.remote, request.expected_url, false, true)
    expect(invoke).toHaveBeenLastCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ operation: expect.objectContaining({ request: expect.objectContaining({ is_background_task: true }) }) }) }))
  })

  it('delivers only active native fetch progress and rejects unavailable streaming before preview', async () => {
    const preview = { remote: 'origin', recurse_submodules: false, basis: 'c'.repeat(64) }
    const invoke = vi.fn().mockResolvedValue(preview)
    let finish: (result: unknown) => void = () => {}
    let deliver: (value: unknown) => void = () => {}
    const invokeWithProgress = vi.fn((_channel: string, callback: (value: unknown) => void) => {
      deliver = callback
      return new Promise(resolve => { finish = resolve })
    })
    const progress = vi.fn()
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, invokeWithProgress, send: vi.fn(), on: vi.fn() } })
    const pending = new BackendClient().fetchRepositoryRemote('/repo', 'origin', 'file:///repo', false, false, undefined, progress)
    await vi.waitFor(() => expect(invokeWithProgress).toHaveBeenCalledOnce())
    deliver({ stream: 'stderr', data: 'aGk=' })
    deliver({ stream: 'invalid', data: 'aGk=' })
    deliver({ stream: 'stderr', data: 'invalid!' })
    expect(progress).toHaveBeenCalledExactlyOnceWith({ stream: 'stderr', data: 'aGk=' })
    finish({ code: 0, error_kind: null })
    await pending
    deliver({ stream: 'stderr', data: 'bGF0ZQ==' })
    expect(progress).toHaveBeenCalledOnce()
    expect(invokeWithProgress).toHaveBeenCalledWith('desktop.backend.git_info', expect.any(Function), expect.objectContaining({ operation: expect.objectContaining({ operation: expect.objectContaining({ request: expect.objectContaining({ report_progress: true }) }) }) }))

    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    invoke.mockClear()
    await expect(new BackendClient().fetchRepositoryRemote('/repo', 'origin', 'file:///repo', false, false, undefined, progress)).rejects.toMatchObject({ code: 'unsupported' })
    expect(invoke).not.toHaveBeenCalled()
  })

  it('reviews tracking branch fast-forwards and never replays an interrupted mutation', async () => {
    const branches = [{ ref: 'refs/heads/feature', sha: 'a'.repeat(40), upstreamRef: 'refs/remotes/origin/feature', upstreamSha: 'b'.repeat(40) }]
    const request = { branches: [{ ref: branches[0].ref, sha: branches[0].sha, upstream_ref: branches[0].upstreamRef, upstream_sha: branches[0].upstreamSha }], recurse_submodules: false }
    const preview = { request, basis: 'c'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ code: 1 })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.fastForwardRepositoryBranches('/repo', branches, false)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'fast_forward_branches', repository_path: '/repo', operation: { method: 'renderer_fast_forward', request, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, request: { ...request, recurse_submodules: true } })
    await expect(client.fastForwardRepositoryBranches('/repo', branches, false)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.fastForwardRepositoryBranches('/repo', branches, false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect tracking branches') })
    expect(invoke).toHaveBeenCalledTimes(5)
  })

  it('reviews mixed reset and never replays an interrupted mutation', async () => {
    const request = { target: 'a'.repeat(40), head: 'b'.repeat(40), branch: 'refs/heads/main', undo: true }
    const preview = { request, basis: 'c'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ target: request.target, code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.mixedResetRepository('/repo', request)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'mixed_reset', repository_path: '/repo', operation: { method: 'renderer_mixed_reset', request, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, request: { ...request, head: 'd'.repeat(40) } })
    await expect(client.mixedResetRepository('/repo', request)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.mixedResetRepository('/repo', request)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect HEAD') })
    expect(invoke).toHaveBeenCalledTimes(5)
  })

  it('reviews merge start and never replays an interrupted mutation', async () => {
    const request = { source_ref: 'refs/remotes/origin/main', source_head: 'a'.repeat(40), current_ref: 'refs/heads/main', current_head: 'b'.repeat(40), squash: true, no_verify: false }
    const preview = { request, basis: 'c'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.startRepositoryMerge('/repo', request)).resolves.toBe('complete')
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'start_merge', repository_path: '/repo', operation: { method: 'renderer_merge_start', request, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, request: { ...request, squash: false } })
    await expect(client.startRepositoryMerge('/repo', request)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.startRepositoryMerge('/repo', request)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect merge state') })
    expect(invoke).toHaveBeenCalledTimes(5)
  })

  it('reviews rebase start and never replays an interrupted mutation', async () => {
    const head = 'a'.repeat(40)
    const request = { base_ref: 'refs/remotes/origin/main', base_head: 'b'.repeat(40), target_ref: 'refs/heads/-topic', target_head: 'c'.repeat(40), current_head: head, current_branch: null }
    const preview = { request, basis: 'd'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.startRepositoryRebase('/repo', request)).resolves.toBe('complete')
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'start_rebase', repository_path: '/repo', operation: { method: 'renderer_rebase_start', request, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, request: { ...request, target_head: head } })
    await expect(client.startRepositoryRebase('/repo', request)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.startRepositoryRebase('/repo', request)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect rebase state') })
    expect(invoke).toHaveBeenCalledTimes(5)
  })

  it('reviews rebase continuation and never replays an interrupted mutation', async () => {
    const head = 'a'.repeat(40)
    const preview = { head, basis: 'b'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.continueRepositoryRebase('/repo', head, true, false)).resolves.toBe('complete')
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'continue_rebase', repository_path: '/repo', operation: { method: 'renderer_rebase_continue', expected: head, basis: preview.basis, empty: true, no_verify: false } } }))
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'conflict', code: 1, stdout: '', stderr: '' })
    await expect(client.continueRepositoryRebase('/repo', head, false, false)).resolves.toBe('conflict')
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.continueRepositoryRebase('/repo', head, false, false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect rebase state') })
    expect(invoke).toHaveBeenCalledTimes(6)
  })

  it('reviews cherry-pick start and never replays an interrupted mutation', async () => {
    const head = 'a'.repeat(40)
    const commits = ['b'.repeat(40), 'c'.repeat(40)]
    const expected = { head, branch: 'refs/heads/main' }
    const preview = { ...expected, commits, basis: 'd'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.startRepositoryCherryPick('/repo', commits, expected)).resolves.toBe('complete')
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'start_cherry_pick', repository_path: '/repo', operation: { method: 'renderer_cherry_pick_start', commits, expected_head: head, expected_branch: expected.branch, basis: preview.basis } } }))
    invoke.mockResolvedValueOnce({ ...preview, commits: [...commits].reverse() })
    await expect(client.startRepositoryCherryPick('/repo', commits, expected)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.startRepositoryCherryPick('/repo', commits, expected)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect cherry-pick state') })
    expect(invoke).toHaveBeenCalledTimes(5)
  })

  it('reviews cherry-pick continuation and never replays an interrupted mutation', async () => {
    const head = 'a'.repeat(40)
    const preview = { head, basis: 'b'.repeat(64) }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.continueRepositoryCherryPick('/repo', head, false)).resolves.toBe('complete')
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'continue_cherry_pick', repository_path: '/repo', operation: { method: 'renderer_cherry_pick_continue', expected: head, basis: preview.basis, empty: false } } }))
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'conflict', code: 1, stdout: '', stderr: '' })
    await expect(client.continueRepositoryCherryPick('/repo', head, false)).resolves.toBe('conflict')
    invoke.mockResolvedValueOnce({ ...preview, head: 'c'.repeat(40) })
    await expect(client.continueRepositoryCherryPick('/repo', head, false)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.continueRepositoryCherryPick('/repo', head, false)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect cherry-pick state') })
    expect(invoke).toHaveBeenCalledTimes(7)
  })

  it('binds revert to the reviewed branch and reports conflicts without replay', async () => {
    const commit = 'a'.repeat(40)
    const head = 'b'.repeat(40)
    const resultHead = 'c'.repeat(40)
    const preview = { basis: 'd'.repeat(64), commit, head, branch: 'refs/heads/main', mainline: null }
    const invoke = vi.fn().mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'complete', code: 0, commit, head_before: head, head_after: resultHead, stdout: '', stderr: '' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const expected = { head, branch: 'refs/heads/main', merge: false }
    await client.revertReviewedCommit('/repo', commit, expected)
    invoke.mockResolvedValueOnce({ ...preview, head: resultHead })
    await expect(client.revertReviewedCommit('/repo', commit, expected)).rejects.toMatchObject({ code: 'revert_changed', retryable: false })
    invoke.mockResolvedValueOnce(preview).mockResolvedValueOnce({ outcome: 'conflict', code: 1, commit, head_before: head, head_after: head, stdout: '', stderr: '' })
    await expect(client.revertReviewedCommit('/repo', commit, expected)).rejects.toMatchObject({ code: 'revert_conflict', retryable: false })
    invoke.mockResolvedValueOnce(preview).mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.revertReviewedCommit('/repo', commit, expected)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('revert state') })
    expect(invoke).toHaveBeenCalledTimes(7)
  })

  it('routes the direct checkout helper through Rust', async () => {
    vi.resetModules()
    const oid = 'b'.repeat(40)
    const invoke = vi.fn().mockResolvedValueOnce({ basis: 'a'.repeat(64), dirty: false, target: { reference: 'refs/heads/feature', oid }, source_branch: 'main', source_head: oid, source_stash: null, occupied_worktree: null }).mockResolvedValueOnce({ code: 0, checked_out: true, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const legacyGit = vi.fn()
    vi.doMock('../../src/lib/git/core', () => ({ git: legacyGit }))
    vi.doMock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: (_path: string, _kind: string, run: () => unknown) => run() }))
    vi.doMock('../../src/lib/git/environment', () => ({ envForRemoteOperation: vi.fn(), getFallbackUrlForProxyResolve: vi.fn() }))
    vi.doMock('../../src/lib/git/submodule', () => ({ updateSubmodulesAfterOperation: vi.fn() }))
    vi.doMock('../../src/lib/progress', () => ({ CheckoutProgressParser: vi.fn(), executionOptionsWithProgress: vi.fn() }))
    const { checkoutBranch, checkoutCommit } = await import('../../src/lib/git/checkout')
    await expect(checkoutBranch({ path: '/repo' } as any, { name: 'feature', ref: 'refs/heads/feature', tip: { sha: oid } } as any, null)).resolves.toBe(true)
    invoke.mockResolvedValueOnce({ basis: 'a'.repeat(64), target: { oid, detached: true } }).mockResolvedValueOnce({ code: 0, checked_out: true, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    await expect(checkoutCommit({ path: '/repo' } as any, { sha: oid } as any, null)).resolves.toBe(true)
    expect(legacyGit).not.toHaveBeenCalled()
    expect(invoke).toHaveBeenCalledTimes(4)
  })

  it('reads complete blobs through the Rust transport', async () => {
    vi.resetModules()
    const oid = 'a'.repeat(40)
    const contents = Buffer.alloc(1024 * 1024 + 3, 0xab)
    const invoke = vi.fn(async (_method: string, request: any): Promise<unknown> => {
      const operation = request.operation.operation
      expect(operation.method).toBe('renderer_blob_chunk')
      expect(operation.expected_oid).toBe(operation.offset === 0 ? null : oid)
      return { oid, size: contents.length, data: contents.subarray(operation.offset, operation.offset + operation.length).toString('base64') }
    })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    vi.doMock('../../src/lib/git/core', () => ({ git: vi.fn(), isMaxBufferExceededError: vi.fn() }))
    vi.doMock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: (_path: string, _kind: string, run: () => unknown) => run() }))
    const { getBlobContents } = await import('../../src/lib/git/show')
    const { Repository } = await import('../../src/models/repository')
    expect((await getBlobContents(new Repository('/tmp', 1, null, false), 'HEAD', 'file')).equals(contents)).toBe(true)
    expect(invoke).toHaveBeenCalledTimes(2)
  }, 30_000)

  it('routes application branch creation, rename and deletion through Rust', async () => {
    vi.resetModules()
    const invoke = vi.fn().mockResolvedValueOnce({ basis: 'a'.repeat(64) }).mockResolvedValueOnce({ code: 0, created: true, checked_out: false, receipt_id: '12345678-1234-4234-8234-123456789abc' })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const legacyGit = vi.fn()
    vi.doMock('../../src/lib/git/core', () => ({ git: legacyGit, isGitError: vi.fn() }))
    vi.doMock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: (_path: string, _kind: string, run: () => unknown) => run() }))
    vi.doMock('../../src/lib/git/environment', () => ({ envForRemoteOperation: vi.fn() }))
    vi.doMock('../../src/lib/git/update-ref', () => ({ deleteRef: vi.fn() }))
    const { createBranch, renameBranch, deleteLocalBranch } = await import('../../src/lib/git/branch')
    await createBranch({ path: '/repo' } as any, 'feature', 'origin/main', true)
    const oid = 'b'.repeat(40)
    const id = '12345678-1234-4234-8234-123456789abc'
    invoke.mockResolvedValueOnce({ basis: 'c'.repeat(64), name: 'feature', new_name: 'renamed', oid }).mockResolvedValueOnce({ code: 0, receipt: { id, name: 'feature', new_name: 'renamed', oid, phase: 'complete' } })
    await renameBranch({ path: '/repo' } as any, { name: 'feature', ref: 'refs/heads/feature', tip: { sha: oid } } as any, 'renamed')
    invoke.mockResolvedValueOnce({ basis: 'c'.repeat(64), branch: { ref: 'refs/heads/feature', oid }, current: false, checkout: null, in_use_worktree: null }).mockResolvedValueOnce({ code: 0, receipt_id: id, backup_ref: `refs/desktop/deleted-branches/${id}` })
    await expect(deleteLocalBranch({ path: '/repo' } as any, 'feature', oid)).resolves.toBe(true)
    expect(invoke).toHaveBeenCalledTimes(6)
    expect(legacyGit).not.toHaveBeenCalled()
  })

  it('validates selected-file size review responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => [false, true])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryLargeFiles('/repo', ['small', 'large'])).resolves.toEqual([false, true])
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ method: 'renderer_large_files', paths: ['small', 'large'] }))
    invoke.mockResolvedValueOnce([true])
    await expect(client.repositoryLargeFiles('/repo', ['small', 'large'])).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce([false, 'true'])
    await expect(client.repositoryLargeFiles('/repo', ['small', 'large'])).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('routes trailer requests and rejects malformed output', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'Summary\n\nReviewed-by: Test\n')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryInterpretTrailers('/repo', 'Summary\n', [{ token: 'Reviewed-by', value: 'Test' }], false, false)).resolves.toContain('Reviewed-by: Test')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'interpret_trailers', repository_path: '/repo', operation: expect.objectContaining({ method: 'interpret_trailers' }) }) }))
    invoke.mockResolvedValueOnce({ output: 'bad' })
    await expect(client.repositoryInterpretTrailers('/repo', 'Summary\n', [], true, false)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('requires complete resource discovery before protecting a repository', async () => {
    const value = { paths: ['/repo', '/repo/.git', '/repo/.git', '/shared/objects', '/repo', '/repo/.git', '/repo/.git', '/shared/objects'], object_directories: ['/shared/objects'], object_routing_diagnostics: '' }
    const invoke = vi.fn(async (): Promise<unknown> => value)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryProtectedResources('/repo')).resolves.toEqual({ paths: value.paths, objectDirectories: value.object_directories, objectRoutingDiagnostics: '' })
    invoke.mockResolvedValueOnce({ ...value, paths: value.paths.slice(0, -1) })
    await expect(client.repositoryProtectedResources('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ ...value, object_directories: ['/other/objects'] })
    await expect(client.repositoryProtectedResources('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates selected commit review without staging', async () => {
    const value = { basis: 'a'.repeat(64), tree: 'c'.repeat(40), head: 'b'.repeat(40), branch: 'refs/heads/main', review: { threshold_bytes: 100 * 1024 * 1024, staged_count: 1, files: ['selected'], large_files: [] } }
    const invoke = vi.fn(async (): Promise<unknown> => value)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySelectedCommitPreview('/repo', { paths: ['selected'] })).resolves.toEqual(value)
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'selected_commit_preview', repository_path: '/repo', operation: expect.objectContaining({ method: 'selected_commit_preview', paths: ['selected'] }) }) }))
    invoke.mockResolvedValueOnce({ ...value, basis: 'bad' })
    await expect(client.repositorySelectedCommitPreview('/repo', { paths: ['selected'] })).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ ...value, tree: 'bad' })
    await expect(client.repositorySelectedCommitPreview('/repo', { paths: ['selected'] })).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ ...value, review: { ...value.review, staged_count: 2 } })
    await expect(client.repositorySelectedCommitPreview('/repo', { paths: ['selected'] })).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ ...value, review: { ...value.review, large_files: [{ path: 'other', display_path: 'other', oid: 'c'.repeat(40), size: 100 * 1024 * 1024 + 1, tracked_by_lfs: false }] } })
    await expect(client.repositorySelectedCommitPreview('/repo', { paths: ['selected'] })).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates commit checkpoint preparation and history', async () => {
    const id = '12345678-1234-4234-8234-123456789abc'
    const basis = 'a'.repeat(64)
    const invoke = vi.fn(async (): Promise<unknown> => ({ id, state: 'prepared', basis }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.prepareSelectedCommit('/repo', { paths: ['file'] }, basis)).resolves.toBe(id)
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'commit_checkpoint', operation: expect.objectContaining({ method: 'prepare_selected_commit', basis }) }) }))
    invoke.mockResolvedValueOnce({ id, state: 'complete', oid: 'b'.repeat(40), stdout: 'done', stderr: 'hook output', output_truncated: false })
    await expect(client.publishSelectedCommit('/repo', id, 'message\n', { noVerify: true, signOff: true })).resolves.toEqual({ oid: 'b'.repeat(40), stdout: 'done', stderr: 'hook output', outputTruncated: false })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'commit_checkpoint', operation: expect.objectContaining({ method: 'publish_selected_commit', checkpoint_id: id, no_verify: true, sign_off: true, allow_large_files: false }) }) }))
    invoke.mockResolvedValueOnce({ id, state: 'complete', oid: 'bad' })
    await expect(client.publishSelectedCommit('/repo', id, 'message')).rejects.toMatchObject({ code: 'invalid_response' })
    await expect(client.publishSelectedCommit('/repo', id, '')).rejects.toMatchObject({ code: 'invalid_request' })
    invoke.mockResolvedValueOnce({ checkpoints: [{ id, state: 'prepared', head: null, branch: 'refs/heads/main' }] })
    await expect(client.listCommitCheckpoints('/repo')).resolves.toHaveLength(1)
    invoke.mockResolvedValueOnce({ checkpoints: [{ id, state: 'published', head: null, branch: 'refs/heads/main' }] })
    await expect(client.listCommitCheckpoints('/repo')).resolves.toEqual([expect.objectContaining({ state: 'published' })])
    invoke.mockResolvedValueOnce({ checkpoints: [{ id: '../bad', state: 'prepared' }] })
    await expect(client.listCommitCheckpoints('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
    const inspection = { id, state: 'prepared', basis, prepared_oid: null, recorded_files_verified: true, index_coverage_verified: true, objects_verified: 1, lfs_verified: 0, shared_indexes_verified: 0, head_matches: true, branch_matches: true, index_matches: true, operation_matches: true }
    invoke.mockResolvedValueOnce(inspection)
    await expect(client.inspectCommitCheckpoint('/repo', id)).resolves.toEqual(inspection)
    invoke.mockResolvedValueOnce({ ...inspection, state: 'publishing' })
    await expect(client.inspectCommitCheckpoint('/repo', id)).resolves.toMatchObject({ state: 'publishing' })
    invoke.mockResolvedValueOnce({ ...inspection, prepared_oid: 'bad' })
    await expect(client.inspectCommitCheckpoint('/repo', id)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ ...inspection, recorded_files_verified: false })
    await expect(client.inspectCommitCheckpoint('/repo', id)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ id, basis, matches: true })
    await expect(client.revalidateCommitCheckpoint('/repo', id)).resolves.toEqual({ basis, matches: true })
    invoke.mockResolvedValueOnce({ id, basis, matches: 'true' })
    await expect(client.revalidateCommitCheckpoint('/repo', id)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ id, state: 'complete', oid: 'b'.repeat(40) })
    await expect(client.recoverPublishedCommitCheckpoint('/repo', id)).resolves.toBe('b'.repeat(40))
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'commit_checkpoint', operation: { method: 'recover_published_commit_checkpoint', checkpoint_id: id } }) }))
    invoke.mockResolvedValueOnce({ id, state: 'complete', oid: 'bad' })
    await expect(client.recoverPublishedCommitCheckpoint('/repo', id)).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({ id, state: 'closed' })
    await expect(client.closeUnchangedCommitCheckpoint('/repo', id)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'commit_checkpoint', operation: { method: 'close_unchanged_commit_checkpoint', checkpoint_id: id } }) }))
    invoke.mockResolvedValueOnce({ id, forgotten: true })
    await expect(client.forgetCommitCheckpoint('/repo', id)).resolves.toBeUndefined()
    await expect(client.forgetCommitCheckpoint('/repo', '../bad')).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates submodule status responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ' abc path (v1)\n')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositorySubmoduleStatus('/repo')).resolves.toBe(' abc path (v1)\n')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'submodule_status', repository_path: '/repo', operation: { method: 'renderer_submodule_status' } } }))
    invoke.mockResolvedValueOnce(null)
    await expect(client.repositorySubmoduleStatus('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates submodule update outcomes', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryUpdateSubmodules('/repo', true)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'update_submodules', repository_path: '/repo', operation: { method: 'renderer_update_submodules', allow_file_protocol: true } } }))
    invoke.mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.repositoryUpdateSubmodules('/repo', false)).rejects.toThrow('inspect submodules')
    invoke.mockResolvedValueOnce({ code: 1, stdout: '', stderr: 'update failed' })
    await expect(client.repositoryUpdateSubmodules('/repo', false)).rejects.toThrow('update failed')
  })

  it('validates submodule reset results and treats interruptions as uncertain', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryResetSubmodules('/repo', ['*-module'])).resolves.toBeUndefined()
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reset_submodules', repository_path: '/repo', operation: { method: 'renderer_reset_submodules', paths: ['*-module'] } } }))
    invoke.mockRejectedValueOnce(new Error('disconnected'))
    await expect(client.repositoryResetSubmodules('/repo', ['*-module'])).rejects.toThrow('inspect the selected submodules')
    invoke.mockResolvedValueOnce({ code: 1, stdout: '', stderr: 'reset failed' })
    await expect(client.repositoryResetSubmodules('/repo', ['*-module'])).rejects.toThrow('reset failed')
    await expect(client.repositoryResetSubmodules('/repo', ['same', 'same'])).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates merge-base requests and responses', async () => {
    const oid = 'a'.repeat(40)
    const invoke = vi.fn(async (): Promise<unknown> => oid)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryMergeBase('/repo', 'HEAD', 'topic')).resolves.toBe(oid)
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'merge_base', repository_path: '/repo', operation: { method: 'merge_base', first: 'HEAD', second: 'topic' } } }))
    invoke.mockResolvedValueOnce(null)
    await expect(client.repositoryMergeBase('/repo', 'missing', 'HEAD')).resolves.toBeNull()
    invoke.mockResolvedValueOnce('invalid')
    await expect(client.repositoryMergeBase('/repo', 'HEAD', 'topic')).rejects.toMatchObject({ code: 'invalid_response' })
    await expect(client.repositoryMergeBase('/repo', '', 'HEAD')).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates merge preview requests and responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ kind: 'conflicts', conflicted_files: 2 }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const ours = 'a'.repeat(40)
    const theirs = 'b'.repeat(40)
    await expect(client.repositoryMergeTreePreview('/repo', ours, theirs)).resolves.toEqual({ kind: 'conflicts', conflictedFiles: 2 })
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: expect.objectContaining({ type: 'merge_tree_preview', operation: { method: 'merge_tree_preview', ours, theirs } }) }))
    invoke.mockResolvedValueOnce({ kind: 'conflicts', conflicted_files: 0 })
    await expect(client.repositoryMergeTreePreview('/repo', ours, theirs)).rejects.toMatchObject({ code: 'invalid_response' })
    await expect(client.repositoryMergeTreePreview('/repo', '--bad', theirs)).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates tag mutation outcomes without retrying uncertain requests', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ code: 0, stdout: '', stderr: '' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const operation = { method: 'tag_create_annotated' as const, name: 'release', target: 'a'.repeat(40) }
    await expect(client.mutateRepositoryTag('/repo', operation)).resolves.toBeUndefined()
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'tag_mutation', repository_path: '/repo', operation } }))
    invoke.mockResolvedValueOnce({ code: 1, stdout: '', stderr: 'tag already exists' })
    await expect(client.mutateRepositoryTag('/repo', operation)).rejects.toThrow('tag already exists')
    invoke.mockResolvedValueOnce({ code: '0' })
    await expect(client.mutateRepositoryTag('/repo', operation)).rejects.toThrow('uncertain')
    await expect(client.mutateRepositoryTag('/repo', { ...operation, target: '\0' })).rejects.toMatchObject({ code: 'invalid_request' })
  })

  it('validates typed history pagination and responses', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => 'YQ==')
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryRendererHistory('/repo', 'HEAD', 10, 2, false, true)).resolves.toBe('YQ==')
    expect(invoke).toHaveBeenCalledWith('desktop.backend.read', expect.objectContaining({ repository_path: '/repo', method: 'renderer_history', commitish: 'HEAD', limit: 10, skip: 2, first_parent: false, exclude_remotes: true }))
    await expect(client.repositoryRendererHistory('/repo', 'HEAD', -1, 0, false, false)).rejects.toMatchObject({ code: 'invalid_request' })
    invoke.mockResolvedValueOnce('bad!')
    await expect(client.repositoryRendererHistory('/repo', 'HEAD', 1, 0, false, false)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates native worktree paths and state', async () => {
    const entry = { path: { bytes: [...Buffer.from('/repo')] }, head: 'abc', branch: 'refs/heads/main', detached: false, locked: null, prunable: null, main: true, bare: false }
    const invoke = vi.fn(async (): Promise<unknown> => [entry])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryWorktrees('/repo')).resolves.toEqual([{ path: '/repo', head: 'abc', branch: 'refs/heads/main', isDetached: false, type: 'main', isLocked: false, isPrunable: false }])
    await expect(client.repositoryWorktrees('/repo/.git/worktrees/missing', true)).resolves.toEqual([{ path: '/repo', head: 'abc', branch: 'refs/heads/main', isDetached: false, type: 'main', isLocked: false, isPrunable: false }])
    expect(invoke).toHaveBeenLastCalledWith('desktop.backend.read', expect.objectContaining({ method: 'worktrees_from_git_dir', repository_path: '/repo/.git/worktrees/missing' }))
    invoke.mockResolvedValueOnce([{ ...entry, path: { bytes: [255] } }])
    await expect(client.repositoryWorktrees('/repo')).rejects.toMatchObject({ code: 'unsupported_path' })
    invoke.mockResolvedValueOnce([{ ...entry, main: false }])
    await expect(client.repositoryWorktrees('/repo')).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('reviews conflict sides and uses the reviewed native resolution', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const basis = 'a'.repeat(64)
    invoke.mockResolvedValueOnce({ basis, stages: [{ stage: 1 }, { stage: 2 }, { stage: 3 }] }).mockResolvedValueOnce({ code: 0, undo_id: '12345678-1234-1234-1234-123456789abc' })
    await client.repositoryResolveManualConflict('/repo', 'file', 'theirs', { ours: true, theirs: true })
    expect(invoke).toHaveBeenNthCalledWith(1, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo', operation: { method: 'conflict_detail', path: 'file' } } }))
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo', operation: { method: 'resolve_conflict', path: 'file', basis, choice: 'theirs' } } }))
    invoke.mockResolvedValueOnce({ basis, stages: [{ stage: 1 }, { stage: 2 }] })
    await expect(client.repositoryResolveManualConflict('/repo', 'file', 'theirs', { ours: true, theirs: true })).rejects.toMatchObject({ code: 'conflict_changed', retryable: false })
    expect(invoke).toHaveBeenCalledTimes(3)
  })

  it('stages reviewed edited conflicts and never retries uncertain outcomes', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const basis = 'b'.repeat(64)
    invoke.mockResolvedValueOnce({ basis, stages: [{ stage: 1 }, { stage: 2 }] }).mockRejectedValueOnce({ code: 'cancelled', message: 'interrupted', retryable: true })
    await expect(client.repositoryResolveManualConflict('/repo', 'file', 'edited', { ours: true, theirs: false })).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('recovery history') })
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo', operation: { method: 'stage_conflict', path: 'file', basis, allow_markers: true } } }))
    expect(invoke).toHaveBeenCalledTimes(2)
  })

  it('uses certified native discard and returns its undo capability', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const selection = { basis: 'diff-basis', lines: [1] }
    const id = '12345678-1234-1234-1234-123456789abc'
    invoke.mockResolvedValueOnce({ basis: 'a'.repeat(64) }).mockResolvedValueOnce({ code: 0, undo_id: id }).mockResolvedValueOnce({ code: 0 })
    await expect(client.repositoryDiscardSelection('/repo-a', 'file', selection)).resolves.toBe(id)
    await client.undoRepositoryDiscard('/repo-a', id)
    expect(invoke).toHaveBeenNthCalledWith(1, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo-a', operation: { method: 'discard_preview', path: 'file', scope: 'working_tree', selection } } }))
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo-a', operation: { method: 'discard', path: 'file', scope: 'working_tree', selection, basis: 'a'.repeat(64) } } }))
    expect(invoke).toHaveBeenNthCalledWith(3, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo-a', operation: { method: 'undo_discard', undo_id: id } } }))
  })

  it('never submits a discard with an invalid preview', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => ({ basis: 'bad' }))
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryDiscardSelection('/repo-a', 'file', { basis: 'diff-basis', lines: [1] })).rejects.toMatchObject({ code: 'invalid_response' })
    expect(invoke).toHaveBeenCalledOnce()
  })

  it('does not replay an interrupted backup deletion', async () => {
    const invoke = vi.fn(async (): Promise<unknown> => { throw { code: 'cancelled', message: 'interrupted', retryable: true } })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const id = '12345678-1234-1234-1234-123456789abc'
    await expect(client.forgetRepositoryDiscard('/repo-a', id)).rejects.toMatchObject({ code: 'cancelled', retryable: false, message: expect.stringContaining('inspect recovery history') })
    expect(invoke).toHaveBeenCalledOnce()
    expect(invoke).toHaveBeenCalledWith('desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo-a', operation: { method: 'forget_discard', undo_id: id } } }))
  })

  it('validates and submits a reviewed multi-file discard', async () => {
    const id = '12345678-1234-1234-1234-123456789abc'
    const invoke = vi.fn(async (): Promise<unknown> => null)
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    invoke.mockResolvedValueOnce({ basis: 'b'.repeat(64), files: [{ path: 'new' }, { path: { bytes: [111, 108, 100] } }] }).mockResolvedValueOnce({ code: 0, undo_id: id })
    const review = await client.repositoryDiscardBatchPreview('/repo', ['new'])
    expect(review).toEqual({ basis: 'b'.repeat(64), files: ['new', 'old'] })
    await expect(client.repositoryDiscardBatch('/repo', ['new'], review.basis)).resolves.toBe(id)
    expect(invoke).toHaveBeenNthCalledWith(2, 'desktop.backend.git_info', expect.objectContaining({ operation: { type: 'reviewed_discard', repository_path: '/repo', operation: { method: 'discard_batch', paths: ['new'], basis: 'b'.repeat(64) } } }))
    invoke.mockResolvedValueOnce({ basis: 'b'.repeat(64), files: [{ path: 'other' }] })
    await expect(client.repositoryDiscardBatchPreview('/repo', ['new'])).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it('validates discard history before presenting recovery actions', async () => {
    const id = '12345678-1234-1234-1234-123456789abc'
    const invoke = vi.fn(async (): Promise<unknown> => [{ id, state: 'prepared', created_ns: 1_700_000_000_000_000_000, purpose: 'discard', files: [{ display_path: 'file' }], lfs_payloads_unavailable: 0 }])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.repositoryDiscardHistory('/repo-a')).resolves.toEqual([{ id, state: 'prepared', createdMs: 1_700_000_000_000, purpose: 'discard', files: ['file'], unavailableLfs: 0 }])
    invoke.mockResolvedValueOnce([{ id, state: 'invalid', created_ns: 0, error: 'private data' }])
    await expect(client.repositoryDiscardHistory('/repo-a')).resolves.toEqual([{ id, state: 'invalid', createdMs: 0, purpose: 'unknown', files: [], unavailableLfs: 0 }])
    invoke.mockResolvedValueOnce([{ id, state: 'applied', created_ns: 0, purpose: 'discard', files: [{ display_path: 4 }], lfs_payloads_unavailable: 0 }])
    await expect(client.repositoryDiscardHistory('/repo-a')).rejects.toMatchObject({ code: 'invalid_response' })
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

  it('rejects unscoped Copilot requests in the desktop shell', async () => {
    const invoke = vi.fn()
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(client.copilotCreateSession()).rejects.toMatchObject({
      code: 'unsupported',
    })
    await expect(client.copilotCloseSession('session-1')).rejects.toMatchObject(
      { code: 'unsupported' }
    )
    expect(invoke).not.toHaveBeenCalled()
  })

  it('forwards the chosen model, effort and timeout for commit generation', async () => {
    const invoke = vi.fn(async () => ({
      summary: 'fix: example',
      description: 'Example',
      basis: 'staged_diff',
    }))
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await client.copilotGenerateForRepository('/repo-a', {
      paths: ['selected.txt'],
      remove_paths: ['old-name.txt'],
      source: 'working_tree',
      partial_selections: [
        {
          path: 'partial.txt',
          original_path: 'old-partial.txt',
          basis: 'reviewed',
          lines: [1, 2],
        },
      ],
      amend_head: 'abc123',
      expected_head: { ref: 'refs/heads/main', sha: 'abc123' },
      rule_descriptions: ['Include an issue number'],
      account: { host: 'github.com', login: 'AliceCase', id: 42 },
      model: 'selected-model',
      reasoning_effort: 'high',
      timeout_ms: 10000,
      provider: {
        type: 'azure',
        baseUrl: 'https://example.invalid',
        bearerToken: 'synthetic-provider-token',
        azure: { apiVersion: '2025-01-01' },
      },
    })
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.copilot',
      expect.objectContaining({
        repository_path: '/repo-a',
        operation: {
          method: 'copilot_generate_commit_message',
          paths: ['selected.txt'],
          remove_paths: ['old-name.txt'],
          source: 'working_tree',
          partial_selections: [
            {
              path: 'partial.txt',
              original_path: 'old-partial.txt',
              basis: 'reviewed',
              lines: [1, 2],
            },
          ],
          amend_head: 'abc123',
          expected_head: { ref: 'refs/heads/main', sha: 'abc123' },
          rule_descriptions: ['Include an issue number'],
          account: { host: 'github.com', login: 'AliceCase', id: 42 },
          model: 'selected-model',
          reasoning_effort: 'high',
          timeout_ms: 10000,
          provider: {
            type: 'azure',
            baseUrl: 'https://example.invalid',
            bearerToken: 'synthetic-provider-token',
            azure: { apiVersion: '2025-01-01' },
          },
        },
        request_id: expect.any(String),
        deadline_ms: 170000,
      })
    )
  })

  it('requests a HEAD-based preview for a byte-preserving path', async () => {
    const invoke = vi.fn(async () => ({
      path: { bytes: [102, 255] },
      basis: 'reviewed',
      kind: 'text',
      can_select: true,
      hunks: [],
    }))
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.copilotCommitDiffForRepository(
        '/repo-a',
        { bytes: [102, 255] },
        {},
        'old-file'
      )
    ).resolves.toMatchObject({ basis: 'reviewed' })
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.copilot',
      expect.objectContaining({
        repository_path: '/repo-a',
        operation: {
          method: 'copilot_commit_diff_detail',
          path: { bytes: [102, 255] },
          original_path: 'old-file',
        },
        request_id: expect.any(String),
        deadline_ms: 170000,
      })
    )
  })

  it('rejects malformed scoped Copilot responses', async () => {
    const invoke = vi.fn(
      async (): Promise<unknown> => ({
        path: { bytes: [256] },
        basis: 'reviewed',
        kind: 'text',
        can_select: true,
        hunks: [],
      })
    )
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const client = new BackendClient()
    await expect(
      client.copilotCommitDiffForRepository('/repo-a', 'bad')
    ).rejects.toMatchObject({ code: 'invalid_response' })
    invoke.mockResolvedValueOnce({
      summary: '',
      description: 'bad',
      basis: 'staged_diff',
    })
    await expect(
      client.copilotGenerateForRepository('/repo-a', {
        account: { host: 'github.com', login: 'AliceCase', id: 42 },
      })
    ).rejects.toMatchObject({ code: 'invalid_response', retryable: false })
  })

  it('scopes and cancels a native Copilot request without replaying it', async () => {
    const invoke = vi.fn(
      (_channel: string, _request: { request_id: string }) =>
        new Promise(() => {})
    )
    const send = vi.fn()
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send },
    })
    const client = new BackendClient()
    const controller = new AbortController()
    const pending = client.copilotGenerateForRepository(
      '/repo-a',
      {
        account: { host: 'github.com', login: 'Alice', id: 42 },
        timeout_ms: 300_000,
      },
      { signal: controller.signal }
    )
    expect(invoke).toHaveBeenCalledWith(
      'desktop.backend.copilot',
      expect.objectContaining({
        repository_path: '/repo-a',
        operation: {
          method: 'copilot_generate_commit_message',
          paths: [],
          account: { host: 'github.com', login: 'Alice', id: 42 },
          timeout_ms: 300_000,
        },
        request_id: expect.any(String),
        deadline_ms: 330_000,
      })
    )
    const request = invoke.mock.calls[0][1] as { request_id: string }
    controller.abort()
    await expect(pending).rejects.toMatchObject({
      code: 'cancelled',
      retryable: false,
    })
    expect(send).toHaveBeenCalledWith('desktop.backend.cancel', {
      request_id: request.request_id,
    })
    expect(invoke).toHaveBeenCalledTimes(1)
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
