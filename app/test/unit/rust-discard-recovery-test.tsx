import * as React from 'react'
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import type { Dispatcher } from '../../src/ui/dispatcher'
import { PopupType } from '../../src/models/popup'

const mocks = vi.hoisted(() => ({
  history: vi.fn(),
  undo: vi.fn(),
  forget: vi.fn(),
  commitHistory: vi.fn(),
  recoverCommit: vi.fn(),
  closeCommit: vi.fn(),
  forgetPreparedCommit: vi.fn(),
  inspectCommit: vi.fn(),
  showPopup: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    repositoryDiscardHistory: mocks.history,
    listCommitCheckpoints: mocks.commitHistory,
    inspectCommitCheckpoint: mocks.inspectCommit,
  }),
}))
vi.mock('../../src/ui/dialog', () => {
  return {
    Dialog: ({ children }: { children: React.ReactNode }) =>
      React.createElement('div', { role: 'dialog' }, children),
    DialogContent: ({ children }: { children: React.ReactNode }) =>
      React.createElement('div', null, children),
    DefaultDialogFooter: () => null,
  }
})
import {
  DiscardRecovery,
  DiscardRecoveryDialog,
} from '../../src/ui/changes/discard-recovery'

const repository = { path: '/repo' } as Repository
const dispatcher = {
  undoRepositoryDiscard: mocks.undo,
  forgetRepositoryDiscard: mocks.forget,
  recoverPublishedCommitCheckpoint: mocks.recoverCommit,
  closeUnchangedCommitCheckpoint: mocks.closeCommit,
  forgetPreparedCommitCheckpoint: mocks.forgetPreparedCommit,
  showPopup: mocks.showPopup,
} as unknown as Dispatcher
const id = '12345678-1234-1234-1234-123456789abc'
const record = {
  id,
  state: 'applied',
  createdMs: 1_700_000_000_000,
  purpose: 'discard',
  files: ['file.txt'],
  unavailableLfs: 0,
}
beforeEach(() => {
  mocks.commitHistory.mockResolvedValue([])
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
  )
})
afterEach(() => {
  cleanup()
  vi.resetAllMocks()
  vi.unstubAllGlobals()
})

describe('discard recovery history', () => {
  it('opens recovery through the application popup stack', () => {
    render(
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
    expect(mocks.showPopup).toHaveBeenCalledWith({
      type: PopupType.RecoveryHistory,
      repository,
    })
  })

  it('shows a prepared commit and requires confirmation before discarding it', async () => {
    mocks.history.mockResolvedValue([])
    mocks.commitHistory
      .mockResolvedValueOnce([
        { id, state: 'prepared', head: null, branch: 'refs/heads/main' },
      ])
      .mockResolvedValueOnce([])
    mocks.forgetPreparedCommit.mockResolvedValue(undefined)
    mocks.inspectCommit.mockResolvedValue({
      id,
      state: 'prepared',
      branch_matches: true,
      head_matches: true,
      index_matches: false,
      operation_matches: true,
    })
    render(
      <DiscardRecoveryDialog
        onDismissed={() => {}}
        repository={repository}
        dispatcher={dispatcher}
      />
    )
    await screen.findByText('Prepared commit was not published')
    expect(screen.getByText(id)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Recover commit' })).toBeNull()
    fireEvent.click(
      screen.getByRole('button', { name: 'Inspect prepared attempt' })
    )
    await screen.findByText(/index changed/)
    expect(mocks.inspectCommit).toHaveBeenCalledWith('/repo', id)
    fireEvent.click(
      screen.getByRole('button', { name: 'Remove prepared attempt' })
    )
    expect(mocks.forgetPreparedCommit).not.toHaveBeenCalled()
    fireEvent.click(
      screen.getByRole('button', { name: 'Discard prepared attempt' })
    )
    await waitFor(() =>
      expect(mocks.forgetPreparedCommit).toHaveBeenCalledWith(repository, id)
    )
    await screen.findByText('No recovery records.')
  })

  it('retains the prepared attempt if the backend rejects deletion', async () => {
    mocks.history.mockResolvedValue([])
    mocks.commitHistory.mockResolvedValue([
      { id, state: 'prepared', head: null, branch: 'refs/heads/main' },
    ])
    mocks.forgetPreparedCommit.mockRejectedValue(
      new Error('Commit checkpoint requires recovery')
    )
    render(
      <DiscardRecoveryDialog
        onDismissed={() => {}}
        repository={repository}
        dispatcher={dispatcher}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Remove prepared attempt' })
    )
    fireEvent.click(
      screen.getByRole('button', { name: 'Discard prepared attempt' })
    )
    expect((await screen.findByRole('alert')).textContent).toContain(
      'requires recovery'
    )
    expect(screen.getByText(id)).toBeTruthy()
  })

  it('closes an unchanged interrupted commit while keeping its backup', async () => {
    mocks.history.mockResolvedValue([])
    mocks.commitHistory
      .mockResolvedValueOnce([
        { id, state: 'publishing', head: null, branch: 'refs/heads/main' },
      ])
      .mockResolvedValueOnce([
        { id, state: 'closed', head: null, branch: 'refs/heads/main' },
      ])
    mocks.closeCommit.mockResolvedValue(undefined)
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Close unchanged attempt' })
    )
    await waitFor(() =>
      expect(mocks.closeCommit).toHaveBeenCalledWith(repository, id)
    )
    await screen.findByText('No recovery records.')
  })
  it('keeps discard recovery available when commit history fails', async () => {
    mocks.history.mockResolvedValue([record])
    mocks.commitHistory.mockRejectedValue(
      new Error('Commit history unavailable')
    )
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Commit history unavailable'
    )
    expect(screen.getByText('file.txt')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Undo' })).toBeTruthy()
  })

  it('keeps commit recovery available when discard history fails', async () => {
    mocks.history.mockRejectedValue(new Error('Discard history unavailable'))
    mocks.commitHistory.mockResolvedValue([
      { id, state: 'published', head: null, branch: 'refs/heads/main' },
    ])
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Discard history unavailable'
    )
    expect(screen.getByRole('button', { name: 'Recover commit' })).toBeTruthy()
  })

  it('recovers an interrupted commit through the dispatcher', async () => {
    mocks.history.mockResolvedValue([])
    mocks.commitHistory
      .mockResolvedValueOnce([
        { id, state: 'publishing', head: null, branch: 'refs/heads/main' },
      ])
      .mockResolvedValueOnce([
        { id, state: 'complete', head: null, branch: 'refs/heads/main' },
      ])
    mocks.recoverCommit.mockResolvedValue(undefined)
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Recover commit' })
    )
    await waitFor(() =>
      expect(mocks.recoverCommit).toHaveBeenCalledWith(repository, id)
    )
    await screen.findByText('No recovery records.')
  })

  it('keeps an interrupted commit when recovery refuses changed state', async () => {
    mocks.history.mockResolvedValue([])
    mocks.commitHistory.mockResolvedValue([
      { id, state: 'published', head: null, branch: 'refs/heads/main' },
    ])
    mocks.recoverCommit.mockRejectedValue(new Error('Index changed'))
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Recover commit' })
    )
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Index changed'
    )
    expect(screen.getByRole('button', { name: 'Recover commit' })).toBeTruthy()
  })
  it('loads durable records and restores through the dispatcher', async () => {
    mocks.history
      .mockResolvedValueOnce([record])
      .mockResolvedValueOnce([{ ...record, state: 'undone' }])
    mocks.undo.mockResolvedValue(undefined)
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    await screen.findByText('file.txt')
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }))
    await waitFor(() => expect(mocks.undo).toHaveBeenCalledWith(repository, id))
    await screen.findByText(/restored/)
    expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull()
  })

  it('keeps interrupted records available when recovery fails', async () => {
    mocks.history.mockResolvedValue([{ ...record, state: 'prepared' }])
    mocks.undo.mockRejectedValue(new Error('File changed since discard'))
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(await screen.findByRole('button', { name: 'Recover' }))
    expect((await screen.findByRole('alert')).textContent).toContain(
      'File changed since discard'
    )
    expect(screen.getByRole('button', { name: 'Recover' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Delete backup' })).toBeNull()
  })

  it('requires explicit confirmation before removing a completed backup', async () => {
    mocks.history.mockResolvedValueOnce([record]).mockResolvedValueOnce([])
    mocks.forget.mockResolvedValue(undefined)
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Delete backup' })
    )
    expect(screen.getByText(/permanently removes/)).toBeTruthy()
    expect(mocks.forget).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(mocks.forget).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Delete backup' }))
    fireEvent.click(screen.getByRole('button', { name: 'Delete permanently' }))
    await waitFor(() => expect(mocks.forget).toHaveBeenCalledOnce())
    expect(mocks.forget).toHaveBeenCalledWith(repository, id)
    await screen.findByText('No recovery records.')
  })

  it('retains the confirmation and error when backup deletion fails', async () => {
    mocks.history.mockResolvedValue([{ ...record, state: 'undone' }])
    mocks.forget.mockRejectedValue(new Error('Backup is busy'))
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Delete backup' })
    )
    fireEvent.click(screen.getByRole('button', { name: 'Delete permanently' }))
    expect((await screen.findByRole('alert')).textContent).toContain(
      'Backup is busy'
    )
    expect(
      screen.getByRole('button', { name: 'Delete permanently' })
    ).toBeTruthy()
  })

  it('does not submit a second deletion while the first is pending', async () => {
    let finish!: () => void
    mocks.history.mockResolvedValue([record])
    mocks.forget.mockImplementation(
      () =>
        new Promise<void>(resolve => {
          finish = resolve
        })
    )
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    fireEvent.click(
      await screen.findByRole('button', { name: 'Delete backup' })
    )
    const confirm = screen.getByRole('button', { name: 'Delete permanently' })
    fireEvent.click(confirm)
    fireEvent.click(confirm)
    expect(mocks.forget).toHaveBeenCalledOnce()
    finish()
    await waitFor(() => expect(mocks.history).toHaveBeenCalledTimes(2))
  })

  it('does not offer mutation for unreadable records', async () => {
    mocks.history.mockResolvedValue([
      { ...record, state: 'invalid', files: [] },
    ])
    render(
      <DiscardRecoveryDialog
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    await screen.findByText(/Unreadable recovery record/)
    expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull()
  })

  it('does not show a previous repository result after switching', async () => {
    let resolveFirst!: (records: unknown[]) => void
    mocks.history
      .mockImplementationOnce(
        () =>
          new Promise(resolve => {
            resolveFirst = resolve
          })
      )
      .mockResolvedValueOnce([])
    const view = render(
      <DiscardRecoveryDialog
        key="first"
        repository={repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    view.rerender(
      <DiscardRecoveryDialog
        key="second"
        repository={{ path: '/other' } as Repository}
        dispatcher={dispatcher}
        onDismissed={() => {}}
      />
    )
    resolveFirst([record])
    await screen.findByText('No recovery records.')
    expect(screen.queryByText('file.txt')).toBeNull()
    expect(mocks.history).toHaveBeenLastCalledWith('/other')
  })
})
