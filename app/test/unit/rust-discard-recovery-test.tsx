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

const mocks = vi.hoisted(() => ({
  history: vi.fn(),
  undo: vi.fn(),
  forget: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ repositoryDiscardHistory: mocks.history }),
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
import { DiscardRecovery } from '../../src/ui/changes/discard-recovery'

const repository = { path: '/repo' } as Repository
const dispatcher = {
  undoRepositoryDiscard: mocks.undo,
  forgetRepositoryDiscard: mocks.forget,
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
  it('loads durable records and restores through the dispatcher', async () => {
    mocks.history
      .mockResolvedValueOnce([record])
      .mockResolvedValueOnce([{ ...record, state: 'undone' }])
    mocks.undo.mockResolvedValue(undefined)
    render(
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
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
      <DiscardRecovery
        key="first"
        repository={repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
    view.rerender(
      <DiscardRecovery
        key="second"
        repository={{ path: '/other' } as Repository}
        dispatcher={dispatcher}
        disabled={false}
      />
    )
    resolveFirst([record])
    fireEvent.click(screen.getByRole('button', { name: 'Recovery history' }))
    await screen.findByText('No recovery records.')
    expect(screen.queryByText('file.txt')).toBeNull()
    expect(mocks.history).toHaveBeenLastCalledWith('/other')
  })
})
