import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

describe('backend Copilot session cleanup', () => {
  afterEach(() => {
    Reflect.deleteProperty(window, '__desktop')
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
