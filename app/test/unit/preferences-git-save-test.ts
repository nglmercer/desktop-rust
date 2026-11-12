import type * as React from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { PreferencesTab } from '../../src/models/preferences'

const mocks = vi.hoisted(() => ({
  setConfig: vi.fn(),
  setBranch: vi.fn(),
}))
vi.mock('keytar', () => ({
  default: {},
  getPassword: vi.fn(),
  setPassword: vi.fn(),
  deletePassword: vi.fn(),
}))
vi.mock('../../src/lib/custom-integration', () => ({
  TargetPathArgument: '$PATH',
  isValidCustomIntegration: vi.fn().mockResolvedValue(false),
}))
vi.mock('../../src/lib/notifications/show-notification', () => ({
  showNotification: vi.fn(),
}))
vi.mock('../../src/lib/notifications/notification-permission', () => ({
  getSystemNotificationsPermission: vi.fn(),
}))
vi.mock('../../src/ui/preferences/notifications', () => ({
  Notifications: () => null,
}))
vi.mock('../../src/lib/git/config', () => ({
  getGlobalConfigValue: vi.fn(),
  setGlobalConfigValue: mocks.setConfig,
}))
vi.mock('../../src/lib/helpers/default-branch', () => ({
  getDefaultBranch: vi.fn(),
  setDefaultBranch: mocks.setBranch,
}))
vi.mock('../../src/lib/trampoline/trampoline-environment', () => ({
  withTrampolineEnv: vi.fn(),
  getIsBackgroundTaskEnvironment: vi.fn(),
  getDesktopAskpassTrampolinePath: vi.fn(),
  getSSHWrapperPath: vi.fn(),
  getTrampolineEnvironmentPath: vi.fn(),
  getTrampolineFallbackAccount: vi.fn(),
  holdCredentialLease: vi.fn(),
  setHasRejectedCredentialsForEndpoint: vi.fn(),
}))
import { Preferences } from '../../src/ui/preferences/preferences'

function createPreferences() {
  const dispatcher = new Proxy(
    { refreshAuthor: vi.fn() },
    {
      get: (target, key) => Reflect.get(target, key) ?? vi.fn(),
    }
  )
  const onDismissed = vi.fn()
  const repository = { path: '/repo' }
  const instance = new Preferences({
    accounts: [],
    dispatcher,
    onDismissed,
    repository,
    selectedCopilotModelsByAccount: {},
  } as unknown as React.ComponentProps<typeof Preferences>)
  vi.spyOn(instance, 'setState').mockImplementation(update => {
    if (typeof update !== 'function' && update !== null) {
      Object.assign(instance.state, update)
    }
  })
  const save = () =>
    Reflect.get(instance, 'onSave').call(instance) as Promise<void>
  return { instance, dispatcher, onDismissed, repository, save }
}

describe('preferences Git settings save', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.setConfig.mockResolvedValue(undefined)
    mocks.setBranch.mockResolvedValue(undefined)
  })

  it('keeps preferences open after a partial identity write and permits retry', async () => {
    const { instance, dispatcher, onDismissed, repository, save } =
      createPreferences()
    Object.assign(instance.state, {
      initialCommitterName: 'Before',
      initialCommitterEmail: 'before@example.invalid',
      committerName: 'After',
      committerEmail: 'after@example.invalid',
      initialDefaultBranch: 'main',
      defaultBranch: 'main',
    })
    mocks.setConfig
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('offline'))
    await save()
    expect(onDismissed).not.toHaveBeenCalled()
    expect(dispatcher.refreshAuthor).toHaveBeenCalledWith(repository)
    expect(instance.state.selectedIndex).toBe(PreferencesTab.Git)
    expect(instance.state.gitSaveError).toContain(
      'Some changes may already have been saved'
    )
    await save()
    expect(instance.state.gitSaveError).toBeUndefined()
    expect(onDismissed).toHaveBeenCalledOnce()
  })

  it('returns to Git settings when the default branch is rejected', async () => {
    const { instance, onDismissed, save } = createPreferences()
    Object.assign(instance.state, {
      initialCommitterName: '',
      committerName: '',
      initialCommitterEmail: '',
      committerEmail: '',
      initialDefaultBranch: 'main',
      defaultBranch: 'bad..branch',
    })
    mocks.setBranch.mockRejectedValueOnce(new Error('invalid default branch'))
    await save()
    expect(onDismissed).not.toHaveBeenCalled()
    expect(instance.state.selectedIndex).toBe(PreferencesTab.Git)
    expect(instance.state.gitSaveError).toContain('Check its name')
    const error = instance.render().props.children[1]
    expect(error.props.children.props.role).toBe('alert')
    Object.assign(instance.state, { defaultBranch: 'trunk' })
    await save()
    expect(mocks.setBranch).toHaveBeenLastCalledWith('trunk')
    expect(onDismissed).toHaveBeenCalledOnce()
  })

  it('does not start a second save while a Git write is pending', async () => {
    const { instance, onDismissed, save } = createPreferences()
    Object.assign(instance.state, {
      initialCommitterName: 'Before',
      committerName: 'After',
      initialCommitterEmail: '',
      committerEmail: '',
      initialDefaultBranch: 'main',
      defaultBranch: 'main',
    })
    let rejectWrite: (reason: Error) => void = () => {}
    mocks.setConfig.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          rejectWrite = reject
        })
    )
    const first = save()
    await save()
    expect(mocks.setConfig).toHaveBeenCalledTimes(1)
    rejectWrite(new Error('offline'))
    await first
    expect(onDismissed).not.toHaveBeenCalled()
    expect(instance.state.gitSaveError).toContain(
      'Some changes may already have been saved'
    )
  })
})
