import { afterEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  reveal: vi.fn(),
  showFolder: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    revealRegisteredRepository: mocks.reveal,
  }),
}))
vi.mock('../../src/ui/main-process-proxy', () => ({
  showFolderContents: mocks.showFolder,
  showItemInFolder: vi.fn(),
  openExternal: vi.fn(),
  moveItemToTrash: vi.fn(),
  writeClipboardText: vi.fn(),
}))
vi.mock('../../src/lib/open-path', () => ({ openPath: vi.fn() }))

import { showRepositoryInFileManager } from '../../src/lib/app-shell'
import { Repository } from '../../src/models/repository'
import { generateRepositoryListContextMenu } from '../../src/ui/repositories-list/repository-list-item-context-menu'
import { RevealInFileManagerLabel } from '../../src/ui/lib/context-menu'

describe('repository file manager routing', () => {
  afterEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it('opens the requested repository without changing it during an async selection', async () => {
    let finish: () => void = () => {}
    mocks.reveal.mockReturnValue(new Promise<void>(resolve => { finish = resolve }))
    const onError = vi.fn()
    const operation = showRepositoryInFileManager('/saved/other repository', onError)
    expect(mocks.reveal).toHaveBeenCalledWith('/saved/other repository')
    mocks.transport = 'wry'
    finish()
    await operation
    expect(mocks.showFolder).not.toHaveBeenCalled()
    expect(onError).not.toHaveBeenCalled()
  })

  it('reports launch errors to the UI', async () => {
    const error = new Error('File manager unavailable')
    mocks.reveal.mockRejectedValue(error)
    const onError = vi.fn()
    await showRepositoryInFileManager('/saved/repo', onError)
    expect(onError).toHaveBeenCalledWith(error)
  })

  it('logs a failed error handler instead of leaving a rejected UI task', async () => {
    mocks.reveal.mockRejectedValue(new Error('File manager unavailable'))
    const reportFailure = new Error('Error handler failed')
    const logger = vi.spyOn(log, 'error')
    try {
      await expect(showRepositoryInFileManager('/saved/repo', async () => {
        throw reportFailure
      })).resolves.toBeUndefined()
      expect(logger).toHaveBeenCalledWith('Failed to report the file manager error', reportFailure)
    } finally {
      logger.mockRestore()
    }
  })

  it('routes the repository context-menu action and reports a failed launch', async () => {
    const repository = new Repository('/saved/context target', 9, null, false)
    const error = new Error('File manager unavailable')
    mocks.reveal.mockRejectedValue(error)
    const onError = vi.fn()
    const noop = () => {}
    const items = generateRepositoryListContextMenu({
      repository,
      shellLabel: undefined,
      externalEditorLabel: undefined,
      askForConfirmationOnRemoveRepository: false,
      accounts: [],
      onViewOnGitHub: noop,
      onOpenInShell: noop,
      onShowRepository: selected => { void showRepositoryInFileManager(selected.path, onError) },
      onOpenInExternalEditor: noop,
      onRemoveRepository: noop,
      onChangeRepositoryAlias: noop,
      onRemoveRepositoryAlias: noop,
      onChangeRepositoryAccount: noop,
      onShowRepositorySettings: noop,
    })
    const reveal = items.find(item => item.label === RevealInFileManagerLabel)
    expect(reveal?.enabled).toBe(true)
    reveal?.action?.()
    await vi.waitFor(() => expect(onError).toHaveBeenCalledWith(error))
    expect(mocks.reveal).toHaveBeenCalledWith('/saved/context target')
  })

  it.each(['wry', 'none'])('keeps the existing %s file manager route', async transport => {
    mocks.transport = transport
    mocks.showFolder.mockResolvedValue(undefined)
    const onError = vi.fn()
    await showRepositoryInFileManager('/saved/repo', onError)
    expect(mocks.showFolder).toHaveBeenCalledWith('/saved/repo')
    expect(mocks.reveal).not.toHaveBeenCalled()
    expect(onError).not.toHaveBeenCalled()
  })
})
