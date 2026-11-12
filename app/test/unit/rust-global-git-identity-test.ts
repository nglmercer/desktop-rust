import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  request: vi.fn(),
  repositoryIdentity: vi.fn(),
  repositorySetting: vi.fn(),
  globalPath: vi.fn(),
  setRepositoryIdentity: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    request: mocks.request,
    repositoryGitIdentityValue: mocks.repositoryIdentity,
    repositoryGitReadSetting: mocks.repositorySetting,
    globalGitConfigPath: mocks.globalPath,
    setRepositoryGitIdentityValue: mocks.setRepositoryIdentity,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
import {
  getGlobalConfigValue,
  getConfigValue,
  getBooleanConfigValue,
  getGlobalConfigPath,
  setConfigValue,
  removeConfigValue,
  setGlobalConfigValue,
} from '../../src/lib/git/config'
import type { Repository } from '../../src/models/repository'

describe('global Git identity reads', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  it('uses typed global settings without an open repository', async () => {
    mocks.request.mockResolvedValue({
      scope: 'global',
      values: { 'user.name': 'Alice', 'user.email': null },
    })
    await expect(getGlobalConfigValue('user.name')).resolves.toBe('Alice')
    await expect(getGlobalConfigValue('user.email')).resolves.toBeNull()
    expect(mocks.request).toHaveBeenCalledWith('git_settings', {
      scope: 'global',
    })
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'reads and writes the default branch on %s',
    async transport => {
      mocks.transport = transport
      mocks.request.mockResolvedValue({
        scope: 'global',
        values: { 'init.defaultbranch': 'main' },
      })
      await expect(getGlobalConfigValue('init.defaultBranch')).resolves.toBe(
        'main'
      )
      await setGlobalConfigValue('init.defaultBranch', 'trunk')
      expect(mocks.request).toHaveBeenNthCalledWith(
        1,
        transport === 'wry' ? 'git_info' : 'git_settings',
        transport === 'wry'
          ? { operation: { type: 'read' } }
          : { scope: 'global' }
      )
      expect(mocks.request).toHaveBeenNthCalledWith(
        2,
        transport === 'wry' ? 'git_info' : 'set_git_setting',
        transport === 'wry'
          ? {
              operation: {
                type: 'set',
                key: 'init.defaultbranch',
                value: 'trunk',
              },
            }
          : { scope: 'global', key: 'init.defaultbranch', value: 'trunk' }
      )
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )
  it('propagates bridge failures and rejects malformed identity responses', async () => {
    const failure = new Error('disconnected')
    mocks.request.mockRejectedValueOnce(failure)
    await expect(getGlobalConfigValue('user.name')).rejects.toBe(failure)
    for (const response of [
      { scope: 'local', values: { 'user.name': 'Alice' } },
      { scope: 'global', values: {} },
      { scope: 'global', values: { 'user.name': 2 } },
    ]) {
      mocks.request.mockResolvedValueOnce(response)
      await expect(getGlobalConfigValue('user.name')).rejects.toThrow(
        'Invalid global Git'
      )
    }
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('preserves custom HOME, other settings, and legacy shell behavior', async () => {
    mocks.git.mockResolvedValue({ exitCode: 0, stdout: 'value\0' })
    await expect(
      getGlobalConfigValue('user.name', { HOME: '/alternate' })
    ).resolves.toBe('value')
    await expect(getGlobalConfigValue('core.editor')).resolves.toBe('value')
    mocks.transport = 'none'
    await expect(getGlobalConfigValue('user.email')).resolves.toBe('value')
    expect(mocks.git).toHaveBeenCalledTimes(3)
    expect(mocks.request).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'uses the Rust global configuration path on %s',
    async transport => {
      mocks.transport = transport
      mocks.globalPath.mockResolvedValue('/home/test/.config/git/config')
      await expect(getGlobalConfigPath()).resolves.toBe(
        '/home/test/.config/git/config'
      )
      expect(mocks.globalPath).toHaveBeenCalledOnce()
      expect(mocks.git).not.toHaveBeenCalled()
      mocks.git.mockResolvedValueOnce({ stdout: '/alternate/.gitconfig' })
      await expect(getGlobalConfigPath({ HOME: '/alternate' })).resolves.toBe(
        '/alternate/.gitconfig'
      )
    }
  )
  it.each(['ws', 'wry'])(
    'reads allowlisted repository settings with Git boolean semantics on %s',
    async transport => {
      mocks.transport = transport
      const repo = { path: '/repo' } as Repository
      mocks.repositorySetting
        .mockResolvedValueOnce('input')
        .mockResolvedValueOnce('true')
        .mockResolvedValueOnce(null)
      await expect(getConfigValue(repo, 'core.autocrlf')).resolves.toBe('input')
      await expect(
        getBooleanConfigValue(repo, 'commit.gpgsign', true)
      ).resolves.toBe(true)
      await expect(
        getBooleanConfigValue(repo, 'commit.gpgsign')
      ).resolves.toBeNull()
      expect(mocks.repositorySetting.mock.calls).toEqual([
        ['/repo', 'core.autocrlf', false, false],
        ['/repo', 'commit.gpgsign', true, true],
        ['/repo', 'commit.gpgsign', false, true],
      ])
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )
  it('writes identity through the typed bridge, including empty values', async () => {
    mocks.request.mockResolvedValue({ scope: 'global', values: {} })
    await setGlobalConfigValue('user.name', 'Alice')
    await setGlobalConfigValue('user.email', '')
    expect(mocks.request).toHaveBeenNthCalledWith(1, 'set_git_setting', {
      scope: 'global',
      key: 'user.name',
      value: 'Alice',
    })
    expect(mocks.request).toHaveBeenNthCalledWith(2, 'set_git_setting', {
      scope: 'global',
      key: 'user.email',
      value: '',
    })
    expect(mocks.git).not.toHaveBeenCalled()
    const failure = new Error('disconnected')
    mocks.request.mockRejectedValueOnce(failure)
    await expect(setGlobalConfigValue('user.name', 'Bob')).rejects.toBe(failure)
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('keeps custom HOME and non-identity writes on their original path', async () => {
    mocks.git.mockResolvedValue({ exitCode: 0, stdout: '' })
    await setGlobalConfigValue('user.name', 'Alice', { HOME: '/alternate' })
    await setGlobalConfigValue('core.editor', 'vi')
    mocks.transport = 'none'
    await setGlobalConfigValue('user.email', 'alice@example.invalid')
    expect(mocks.git).toHaveBeenCalledTimes(3)
    expect(mocks.request).not.toHaveBeenCalled()
  })
  it('preserves alternate HOME and unrelated local settings on the legacy route', async () => {
    mocks.git.mockResolvedValue({ exitCode: 0, stdout: '' })
    const repo = { path: '/repo' } as Repository
    await setConfigValue(repo, 'user.name', 'Alice', { HOME: '/alternate' })
    await removeConfigValue(repo, 'user.email', { HOME: '/alternate' })
    await setConfigValue(repo, 'core.editor', 'vi')
    expect(mocks.git).toHaveBeenCalledTimes(3)
    expect(mocks.setRepositoryIdentity).not.toHaveBeenCalled()
  })
  it('uses scoped native identity operations without a repository', async () => {
    mocks.transport = 'wry'
    mocks.request.mockResolvedValue({
      scope: 'global',
      values: { 'user.name': 'Alice', 'user.email': null },
    })
    await expect(getGlobalConfigValue('user.name')).resolves.toBe('Alice')
    await setGlobalConfigValue('user.email', '')
    expect(mocks.request).toHaveBeenNthCalledWith(1, 'git_info', {
      operation: { type: 'read' },
    })
    expect(mocks.request).toHaveBeenNthCalledWith(2, 'git_info', {
      operation: { type: 'set', key: 'user.email', value: '' },
    })
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'writes and removes repository identity on %s',
    async transport => {
      mocks.transport = transport
      mocks.setRepositoryIdentity.mockResolvedValue(undefined)
      const repo = { path: '/repo' } as Repository
      await setConfigValue(repo, 'user.name', '')
      await removeConfigValue(repo, 'user.email')
      expect(mocks.setRepositoryIdentity).toHaveBeenNthCalledWith(
        1,
        '/repo',
        'user.name',
        ''
      )
      expect(mocks.setRepositoryIdentity).toHaveBeenNthCalledWith(
        2,
        '/repo',
        'user.email',
        null
      )
      const failure = new Error('disconnected')
      mocks.setRepositoryIdentity.mockRejectedValueOnce(failure)
      await expect(setConfigValue(repo, 'user.name', 'Alice')).rejects.toBe(
        failure
      )
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )
  it.each(['ws', 'wry'])(
    'reads repository identity on %s with the requested scope',
    async transport => {
      mocks.transport = transport
      mocks.repositoryIdentity
        .mockResolvedValueOnce(null)
        .mockResolvedValueOnce('Inherited')
      const repo = { path: '/repo' } as Repository
      await expect(getConfigValue(repo, 'user.name', true)).resolves.toBeNull()
      await expect(getConfigValue(repo, 'user.email')).resolves.toBe(
        'Inherited'
      )
      expect(mocks.repositoryIdentity).toHaveBeenNthCalledWith(
        1,
        '/repo',
        'user.name',
        true
      )
      expect(mocks.repositoryIdentity).toHaveBeenNthCalledWith(
        2,
        '/repo',
        'user.email',
        false
      )
      expect(mocks.git).not.toHaveBeenCalled()
      const failure = new Error('disconnected')
      mocks.repositoryIdentity.mockRejectedValueOnce(failure)
      await expect(getConfigValue(repo, 'user.name')).rejects.toBe(failure)
      expect(mocks.git).not.toHaveBeenCalled()
    }
  )
})
