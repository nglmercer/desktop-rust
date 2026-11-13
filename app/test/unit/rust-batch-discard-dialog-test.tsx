import * as React from 'react'
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
} from '../../src/models/status'
import { DiffSelection, DiffSelectionType } from '../../src/models/diff'
import type { Dispatcher } from '../../src/ui/dispatcher'

const mocks = vi.hoisted(() => ({ preview: vi.fn(), discard: vi.fn() }))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'wry',
    repositoryDiscardBatchPreview: mocks.preview,
  }),
}))
vi.mock('../../src/ui/dialog', () => ({
  Dialog: ({
    children,
    onSubmit,
  }: {
    children: React.ReactNode
    onSubmit: () => void
  }) =>
    React.createElement(
      'form',
      {
        onSubmit: (event: React.FormEvent) => {
          event.preventDefault()
          onSubmit()
        },
      },
      children
    ),
  DialogContent: ({ children }: { children: React.ReactNode }) =>
    React.createElement('div', null, children),
  DialogFooter: ({ children }: { children: React.ReactNode }) =>
    React.createElement('div', null, children),
}))
vi.mock('../../src/ui/dialog/ok-cancel-button-group', () => ({
  OkCancelButtonGroup: ({
    okButtonText,
    okButtonDisabled,
  }: {
    okButtonText: string
    okButtonDisabled: boolean
  }) =>
    React.createElement(
      'button',
      { type: 'submit', disabled: okButtonDisabled },
      okButtonText
    ),
}))
vi.mock('../../src/ui/lib/path-text', () => ({
  PathText: ({ path }: { path: string }) =>
    React.createElement('span', null, path),
}))
vi.mock('../../src/ui/lib/checkbox', () => ({
  Checkbox: () => null,
  CheckboxValue: { Off: 0, On: 1 },
}))
vi.mock('../../src/ui/lib/context-menu', () => ({ TrashNameLabel: 'Trash' }))
import { DiscardChanges } from '../../src/ui/discard-changes/discard-changes-dialog'

const repository = { path: '/repo' } as Repository
const files = [
  new WorkingDirectoryFileChange(
    'new',
    { kind: AppFileStatusKind.Untracked },
    DiffSelection.fromInitialSelection(DiffSelectionType.All)
  ),
]
const dispatcher = { discardChanges: mocks.discard } as unknown as Dispatcher
afterEach(() => {
  cleanup()
  vi.resetAllMocks()
})

describe('batch discard review', () => {
  it('requires a completed review and shows additional affected paths', async () => {
    let resolve!: (value: unknown) => void
    mocks.preview.mockReturnValue(
      new Promise(done => {
        resolve = done
      })
    )
    mocks.discard.mockResolvedValue(undefined)
    const dismissed = vi.fn()
    render(
      <DiscardChanges
        repository={repository}
        dispatcher={dispatcher}
        files={files}
        confirmDiscardChanges={true}
        discardingAllChanges={false}
        showDiscardChangesSetting={false}
        onDismissed={dismissed}
        onConfirmDiscardChangesChanged={vi.fn()}
      />
    )
    expect(
      (
        screen.getByRole('button', {
          name: /Discard changes/i,
        }) as HTMLButtonElement
      ).disabled
    ).toBe(true)
    resolve({ basis: 'reviewed-basis', files: ['new', 'old'] })
    await screen.findByText('old')
    expect(screen.getByText(/Recovery history/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /Discard changes/i }))
    await waitFor(() =>
      expect(mocks.discard).toHaveBeenCalledWith(
        repository,
        files,
        true,
        'reviewed-basis'
      )
    )
    await waitFor(() => expect(dismissed).toHaveBeenCalledOnce())
  })

  it('does not submit when the review fails', async () => {
    mocks.preview.mockRejectedValue(new Error('File changed'))
    render(
      <DiscardChanges
        repository={repository}
        dispatcher={dispatcher}
        files={files}
        confirmDiscardChanges={true}
        discardingAllChanges={false}
        showDiscardChangesSetting={false}
        onDismissed={vi.fn()}
        onConfirmDiscardChangesChanged={vi.fn()}
      />
    )
    expect((await screen.findByRole('alert')).textContent).toContain(
      'File changed'
    )
    expect(
      (
        screen.getByRole('button', {
          name: /Discard changes/i,
        }) as HTMLButtonElement
      ).disabled
    ).toBe(true)
    expect(mocks.discard).not.toHaveBeenCalled()
  })
})
