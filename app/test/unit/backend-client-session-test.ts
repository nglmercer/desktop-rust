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
