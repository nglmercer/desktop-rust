import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  request: vi.fn(),
  repositoryIdentity: vi.fn(),
  repositorySetting: vi.fn(),
  setRepositoryIdentity: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    request: mocks.request,
    repositoryGitIdentityValue: mocks.repositoryIdentity,
    repositoryGitReadSetting: mocks.repositorySetting,
    setRepositoryGitIdentityValue: mocks.setRepositoryIdentity,
    globalGitSettingValue: vi.fn(async key => {
      const result = await mocks.request(
        mocks.transport === 'wry' ? 'git_info' : 'git_settings',
        mocks.transport === 'wry'
          ? { operation: { type: 'read' } }
          : { scope: 'global' }
      )
      return result.values[key] ?? null
    }),
    setGlobalGitSetting: vi.fn(async (key, value) => {
      await mocks.request(
        mocks.transport === 'wry' ? 'git_info' : 'set_git_setting',
        mocks.transport === 'wry'
          ? { operation: { type: 'set', key, value } }
          : { scope: 'global', key, value }
      )
    }),
  }),
}))
import {
  getGlobalConfigValue,
  getConfigValue,
  getBooleanConfigValue,
  setConfigValue,
  removeConfigValue,
  setGlobalConfigValue,
} from '../../src/lib/git/config'
import type { Repository } from '../../src/models/repository'

describe('typed Git configuration routing', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it.each(['ws', 'wry'])(
    'reads and writes allowlisted global settings on %s',
    async transport => {
      mocks.transport = transport
      mocks.request.mockResolvedValue({
        scope: 'global',
        values: { 'init.defaultbranch': 'main', 'user.name': 'Alice' },
      })
      await expect(getGlobalConfigValue('init.defaultBranch')).resolves.toBe(
        'main'
      )
      await expect(getGlobalConfigValue('user.name')).resolves.toBe('Alice')
      await setGlobalConfigValue('init.defaultBranch', 'trunk')
      expect(mocks.request).toHaveBeenCalledWith(
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
    }
  )

  it('rejects arbitrary global settings instead of falling back to shell Git', async () => {
    await expect(getGlobalConfigValue('core.editor')).rejects.toThrow(
      'Unsupported global Git setting'
    )
    await expect(setGlobalConfigValue('core.editor', 'vi')).rejects.toThrow(
      'Unsupported global Git setting'
    )
    expect(mocks.request).not.toHaveBeenCalled()
  })

  it('reads allowlisted repository settings with Git boolean semantics', async () => {
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
  })

  it('rejects arbitrary repository settings instead of falling back to shell Git', async () => {
    const repo = { path: '/repo' } as Repository
    await expect(getConfigValue(repo, 'core.editor')).rejects.toThrow(
      'Unsupported repository Git setting'
    )
    await expect(setConfigValue(repo, 'core.editor', 'vi')).rejects.toThrow(
      'Unsupported repository Git setting'
    )
    expect(mocks.repositorySetting).not.toHaveBeenCalled()
    expect(mocks.setRepositoryIdentity).not.toHaveBeenCalled()
  })

  it.each(['ws', 'wry'])(
    'reads, writes, and removes repository identity on %s',
    async transport => {
      mocks.transport = transport
      mocks.repositoryIdentity
        .mockResolvedValueOnce(null)
        .mockResolvedValueOnce('Inherited')
      mocks.setRepositoryIdentity.mockResolvedValue(undefined)
      const repo = { path: '/repo' } as Repository
      await expect(getConfigValue(repo, 'user.name', true)).resolves.toBeNull()
      await expect(getConfigValue(repo, 'user.email')).resolves.toBe(
        'Inherited'
      )
      await setConfigValue(repo, 'user.name', '')
      await removeConfigValue(repo, 'user.email')
      expect(mocks.repositoryIdentity.mock.calls).toEqual([
        ['/repo', 'user.name', true],
        ['/repo', 'user.email', false],
      ])
      expect(mocks.setRepositoryIdentity.mock.calls).toEqual([
        ['/repo', 'user.name', ''],
        ['/repo', 'user.email', null],
      ])
    }
  )
})
