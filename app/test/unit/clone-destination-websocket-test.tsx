import * as React from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const status = vi.hoisted(() => vi.fn())
const canonical = vi.hoisted(() => vi.fn(async (path: string) => path))
vi.mock('electron', () => ({
  ipcRenderer: { send: vi.fn(), on: vi.fn(), invoke: vi.fn() },
  shell: { openPath: vi.fn(async () => '') },
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    cloneDestinationStatus: status,
    canonicalMutationPath: canonical,
  }),
}))

import { CloneRepository } from '../../src/ui/clone-repository/clone-repository'
import { CloneRepositoryTab } from '../../src/models/clone-repository-tab'
import { Account } from '../../src/models/account'
import type { Dispatcher } from '../../src/ui/dispatcher'
import { fireEvent, render, screen, waitFor } from '../helpers/ui/vitest-render'
import { setDefaultDir } from '../../src/ui/lib/default-dir'

const messages = {
  not_empty: 'This folder contains files. Git can only clone to empty folders.',
  not_directory:
    'There is already a file with this name. Git can only clone to a folder.',
  dangling_symlink:
    'A broken symbolic link already exists at this path. Choose a different folder.',
  unreadable:
    'Unable to read path on disk. Please check the path and try again.',
}

function renderDialog(tab = CloneRepositoryTab.Generic) {
  const dispatcher = { clone: vi.fn(), closeFoldout: vi.fn() }
  const accounts =
    tab === CloneRepositoryTab.GitHub
      ? [
          new Account(
            'alice',
            'https://api.github.com',
            'token',
            [],
            '',
            1,
            'Alice'
          ),
        ]
      : []
  const view = render(
    <CloneRepository
      dispatcher={dispatcher as unknown as Dispatcher}
      onDismissed={vi.fn()}
      accounts={accounts}
      initialURL="https://example.com/owner/repository.git"
      selectedTab={tab}
      selectedAccount={null}
      onTabSelected={vi.fn()}
      apiRepositories={new Map()}
      onRefreshRepositories={vi.fn()}
      isTopMost={true}
    />
  )
  const input = screen.getByRole('textbox', {
    name: /Local path/i,
    hidden: true,
  })
  const button = screen.getByRole('button', { name: 'Clone', hidden: true })
  return { ...view, input, button, dispatcher }
}

describe('WebSocket clone destination validation', () => {
  beforeEach(() => {
    status.mockReset()
    canonical.mockClear()
    status.mockResolvedValue('missing')
    setDefaultDir('/tmp/clone-ui')
  })

  for (const state of [
    'not_empty',
    'not_directory',
    'dangling_symlink',
    'unreadable',
  ] as const) {
    it(`shows the ${state} error and disables cloning`, async () => {
      const view = renderDialog()
      await waitFor(() =>
        expect(view.button.getAttribute('aria-disabled')).toBeNull()
      )
      status.mockResolvedValue(state)
      fireEvent.change(view.input, { target: { value: `/tmp/${state}` } })
      await waitFor(() =>
        expect(screen.getByText(messages[state])).toBeTruthy()
      )
      expect(view.button.getAttribute('aria-disabled')).toBe('true')
      expect(status).toHaveBeenCalledWith(`/tmp/${state}`)
      expect(view.dispatcher.clone).not.toHaveBeenCalled()
    })
  }

  it('disables cloning when the WebSocket check disconnects', async () => {
    const view = renderDialog()
    await waitFor(() =>
      expect(view.button.getAttribute('aria-disabled')).toBeNull()
    )
    status.mockRejectedValue(new Error('disconnected'))
    fireEvent.change(view.input, { target: { value: '/tmp/disconnected' } })
    await waitFor(() =>
      expect(screen.getByText(messages.unreadable)).toBeTruthy()
    )
    expect(view.button.getAttribute('aria-disabled')).toBe('true')
    expect(view.dispatcher.clone).not.toHaveBeenCalled()
  })

  it('keeps selected-account validation when an earlier mode responds later', async () => {
    const view = renderDialog(CloneRepositoryTab.GitHub)
    await waitFor(() =>
      expect((view.input as HTMLInputElement).value).toBe(
        '/tmp/clone-ui/repository'
      )
    )
    let finishOld!: (value: string) => void
    let count = 0
    status.mockImplementation((path: string) => {
      if (path !== '/tmp/old-mode') {
        return Promise.resolve('missing')
      }
      count++
      return count === 1
        ? new Promise(resolve => {
            finishOld = resolve
          })
        : Promise.resolve('empty')
    })
    fireEvent.change(view.input, { target: { value: '/tmp/old-mode' } })
    await waitFor(() => expect(finishOld).toBeTypeOf('function'))
    fireEvent.click(
      screen.getByRole('checkbox', {
        name: /Clone with the selected account/i,
        hidden: true,
      })
    )
    const error = 'This mode requires a destination that does not yet exist.'
    await waitFor(() => expect(screen.getByText(error)).toBeTruthy())
    finishOld('missing')
    await new Promise(resolve => setTimeout(resolve, 10))
    expect(screen.getByText(error)).toBeTruthy()
    expect(view.button.getAttribute('aria-disabled')).toBe('true')
  })

  it('accepts an empty directory and ignores a stale error from an earlier path', async () => {
    const view = renderDialog()
    await waitFor(() =>
      expect(view.button.getAttribute('aria-disabled')).toBeNull()
    )
    let finishOld!: (value: string) => void
    status.mockImplementation((path: string) =>
      path === '/tmp/old'
        ? new Promise(resolve => {
            finishOld = resolve
          })
        : Promise.resolve('empty')
    )
    fireEvent.change(view.input, { target: { value: '/tmp/old' } })
    await waitFor(() => expect(finishOld).toBeTypeOf('function'))
    fireEvent.change(view.input, { target: { value: '/tmp/new' } })
    await waitFor(() => expect(status).toHaveBeenCalledWith('/tmp/new'))
    await waitFor(() =>
      expect(view.button.getAttribute('aria-disabled')).toBeNull()
    )
    finishOld('not_empty')
    await new Promise(resolve => setTimeout(resolve, 10))
    expect(screen.queryByText(messages.not_empty)).toBeNull()
    expect(view.button.getAttribute('aria-disabled')).toBeNull()
  })

  it('dispatches the clone for an empty destination after rechecking it', async () => {
    status.mockResolvedValue('empty')
    const view = renderDialog()
    await waitFor(() =>
      expect(view.button.getAttribute('aria-disabled')).toBeNull()
    )
    fireEvent.change(view.input, { target: { value: '/tmp/empty' } })
    await waitFor(() => expect(status).toHaveBeenCalledWith('/tmp/empty'))
    fireEvent.click(view.button)
    await waitFor(() =>
      expect(view.dispatcher.clone).toHaveBeenCalledWith(
        'https://example.com/owner/repository.git',
        '/tmp/empty',
        { defaultBranch: undefined }
      )
    )
    expect(
      status.mock.calls.filter(([path]) => path === '/tmp/empty').length
    ).toBeGreaterThanOrEqual(3)
  })
})
