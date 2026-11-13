import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Repository } from '../../src/models/repository'
import { PopupType } from '../../src/models/popup'

vi.mock('fs-admin', () => ({}))
vi.mock('keytar', () => ({ default: {} }))

const mocks = vi.hoisted(() => ({
  gitVersion: vi.fn(),
  isGitOnPath: vi.fn(),
  terminals: vi.fn(),
  launch: vi.fn(),
  transport: 'ws' as 'ws' | 'wry',
}))
vi.mock('../../src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../src/backend/client')>()),
  backend: () => ({
    transportKind: mocks.transport,
    gitVersion: mocks.gitVersion,
    availableRustTerminals: mocks.terminals,
    openRepositoryTerminal: mocks.launch,
  }),
}))
vi.mock('../../src/lib/is-git-on-path', () => ({
  isGitOnPath: mocks.isGitOnPath,
}))

import { Dispatcher } from '../../src/ui/dispatcher/dispatcher'
import { AppStore } from '../../src/lib/stores/app-store'

describe('Rust terminal dispatch', () => {
  beforeEach(() => {
    vi.stubGlobal('__LINUX__', true)
    mocks.gitVersion.mockReset()
    mocks.transport = 'ws'
    mocks.isGitOnPath.mockReset()
    mocks.terminals.mockReset()
    mocks.launch.mockReset().mockResolvedValue(undefined)
  })
  afterEach(() => vi.unstubAllGlobals())

  it('queries Rust for Git and preserves the explicitly requested repository', async () => {
    const repository = { path: '/repo-b' } as Repository
    const store = { _openShell: vi.fn(), _showPopup: vi.fn() }
    const dispatcher = Object.assign(Object.create(Dispatcher.prototype), {
      appStore: store,
    }) as Dispatcher
    mocks.gitVersion.mockResolvedValue('git version 2')
    await dispatcher.openShell('/repo-b', false, repository)
    expect(mocks.gitVersion).toHaveBeenCalledOnce()
    expect(mocks.isGitOnPath).not.toHaveBeenCalled()
    expect(store._openShell).toHaveBeenCalledWith('/repo-b', repository)
    expect(store._showPopup).not.toHaveBeenCalled()
  })

  it('preserves the repository when showing and bypassing the Git warning', async () => {
    const repository = { path: '/repo-b' } as Repository
    const store = { _openShell: vi.fn(), _showPopup: vi.fn() }
    const dispatcher = Object.assign(Object.create(Dispatcher.prototype), {
      appStore: store,
    }) as Dispatcher
    mocks.gitVersion.mockRejectedValue(new Error('Git unavailable'))
    await dispatcher.openShell('/repo-b', false, repository)
    expect(store._showPopup).toHaveBeenCalledWith({
      type: PopupType.InstallGit,
      path: '/repo-b',
      repository,
    })
    await dispatcher.openShell('/repo-b', true, repository)
    expect(store._openShell).toHaveBeenCalledWith('/repo-b', repository)
    expect(mocks.isGitOnPath).not.toHaveBeenCalled()
  })

  it('retains the previous Git check on other platforms', async () => {
    vi.stubGlobal('__LINUX__', false)
    const store = { _openShell: vi.fn(), _showPopup: vi.fn() }
    const dispatcher = Object.assign(Object.create(Dispatcher.prototype), {
      appStore: store,
    }) as Dispatcher
    mocks.isGitOnPath.mockResolvedValue(true)
    await dispatcher.openShell('/repo')
    expect(mocks.isGitOnPath).toHaveBeenCalledOnce()
    expect(mocks.gitVersion).not.toHaveBeenCalled()
    expect(store._openShell).toHaveBeenCalledWith('/repo', undefined)
  })

  it.each(['ws', 'wry'] as const)(
    'opens the requested repository via %s even if a different one is selected',
    async transport => {
      mocks.transport = transport
      const selected = new Repository('/repo-a', 1, null, false)
      const requested = new Repository('/repo-b', 2, null, false)
      const emitError = vi.fn()
      const store = Object.assign(Object.create(AppStore.prototype), {
        statsStore: { increment: vi.fn() },
        selectedRepository: selected,
        selectedShell: 'Konsole',
        getState: () => ({ useCustomShell: false, customShell: null }),
        emitError,
      }) as AppStore
      mocks.terminals.mockResolvedValue([
        { name: 'Konsole', path: '/usr/bin/konsole' },
      ])
      await store._openShell('/repo-b/nested', requested)
      expect(mocks.launch).toHaveBeenCalledWith(
        '/repo-b',
        'Konsole',
        '/repo-b/nested'
      )
      expect(emitError).not.toHaveBeenCalled()
    }
  )
})
