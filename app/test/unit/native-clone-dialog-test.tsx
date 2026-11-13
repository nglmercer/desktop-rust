import * as React from 'react'
import { act } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '../helpers/ui/vitest-render'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  pathExists: vi.fn(),
  readdir: vi.fn(),
  clone: vi.fn(),
  recoveries: vi.fn(),
}))

vi.mock('fs/promises', async importOriginal => ({
  ...(await importOriginal<typeof import('fs/promises')>()),
  readdir: mocks.readdir,
}))
vi.mock('../../src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../src/backend/client')>()),
  backend: () => ({
    transportKind: mocks.transport,
    pathExists: mocks.pathExists,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', async importOriginal => ({
  ...(await importOriginal<
    typeof import('../../src/lib/git/repository-operation')
  >()),
  canonicalMutationPath: async (path: string) => path,
  isRepositoryAffectedByAssistedCommit: () => false,
}))
vi.mock('../../src/ui/lib/default-dir', () => ({
  getDefaultDir: async () => '/clones',
  setDefaultDir: vi.fn(),
}))
vi.mock('../../src/ui/clone-repository/native-clone', async importOriginal => ({
  ...(await importOriginal<
    typeof import('../../src/ui/clone-repository/native-clone')
  >()),
  listNativeCloneRecoveries: mocks.recoveries,
  nativeClone: mocks.clone,
}))
vi.mock('../../src/ui/clone-repository/clone-github-repository', () => ({
  CloneGithubRepository: ({
    path,
    onPathChanged,
  }: {
    path: string
    onPathChanged: (value: string) => void
  }) => (
    <input
      aria-label="Local path"
      value={path}
      onChange={event => onPathChanged(event.currentTarget.value)}
    />
  ),
}))
vi.mock('../../src/ui/dialog', () => ({
  Dialog: ({
    children,
    onSubmit,
    onDismissed,
    dismissDisabled,
  }: {
    children: React.ReactNode
    onSubmit: () => void
    onDismissed: () => void
    dismissDisabled?: boolean
  }) => (
    <form
      onSubmit={event => {
        event.preventDefault()
        onSubmit()
      }}
    >
      <button type="button" onClick={onDismissed} disabled={dismissDisabled}>
        Close
      </button>
      {children}
    </form>
  ),
  DialogContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogError: ({ children }: { children: React.ReactNode }) => (
    <div role="alert">{children}</div>
  ),
  DialogFooter: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))
vi.mock('../../src/ui/dialog/ok-cancel-button-group', () => ({
  OkCancelButtonGroup: ({
    okButtonDisabled,
  }: {
    okButtonDisabled: boolean
  }) => (
    <button type="submit" disabled={okButtonDisabled}>
      Clone
    </button>
  ),
}))

import { Account } from '../../src/models/account'
import { CloneRepositoryTab } from '../../src/models/clone-repository-tab'
import { API } from '../../src/lib/api'
import { CloneRepository } from '../../src/ui/clone-repository/clone-repository'
import type { Dispatcher } from '../../src/ui/dispatcher'

describe.each(['ws', 'wry'])('native clone dialog over %s', transport => {
  beforeEach(() => {
    mocks.transport = transport
    mocks.pathExists.mockReset().mockResolvedValue(false)
    mocks.readdir
      .mockReset()
      .mockRejectedValue(new Error('legacy filesystem unavailable'))
    mocks.recoveries.mockReset().mockResolvedValue([])
    mocks.clone
      .mockReset()
      .mockResolvedValue({ path: '/clones/final', staging: '/clones/.stage' })
    vi.spyOn(API, 'fromAccount').mockReturnValue({
      fetchRepositoryCloneInfo: async () => ({
        url: 'https://github.com/alice/project.git',
      }),
    } as unknown as API)
  })

  afterEach(() => vi.restoreAllMocks())

  it('checks the selected path through the native backend and submits the selected account', async () => {
    const account = new Account(
      'alice',
      'https://api.github.com',
      'token',
      [],
      '',
      12,
      ''
    )
    render(
      <CloneRepository
        dispatcher={
          { closeFoldout: vi.fn(), clone: vi.fn() } as unknown as Dispatcher
        }
        onDismissed={vi.fn()}
        accounts={[account]}
        initialURL="https://github.com/alice/project.git"
        selectedTab={CloneRepositoryTab.GitHub}
        selectedAccount={account}
        onTabSelected={vi.fn()}
        apiRepositories={new Map()}
        onRefreshRepositories={vi.fn()}
        isTopMost={true}
      />
    )

    const path = await screen.findByRole('textbox', { name: 'Local path' })
    await waitFor(() => expect(path).toHaveProperty('value', '/clones/project'))
    const native = screen.getByRole('checkbox', {
      name: /Clone with the selected account/,
    })
    fireEvent.click(native)
    await waitFor(() =>
      expect(mocks.pathExists).toHaveBeenCalledWith('/clones/project')
    )
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Clone' })).not.toHaveProperty(
        'disabled',
        true
      )
    )

    mocks.readdir.mockClear()
    mocks.pathExists.mockResolvedValue(true)
    fireEvent.change(path, { target: { value: '/clones/existing' } })
    await screen.findByRole('alert')
    expect(screen.getByRole('alert').textContent).toContain(
      'destination that does not yet exist'
    )
    expect(screen.getByRole('button', { name: 'Clone' })).toHaveProperty(
      'disabled',
      true
    )
    expect(mocks.readdir).not.toHaveBeenCalled()

    mocks.pathExists.mockResolvedValue(false)
    fireEvent.change(path, { target: { value: '/clones/final' } })
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Clone' })).not.toHaveProperty(
        'disabled',
        true
      )
    )
    let finishClone!: (result: { path: string; staging: string }) => void
    mocks.clone.mockImplementation(
      () => new Promise(resolve => (finishClone = resolve))
    )
    let finishValidation!: (exists: boolean) => void
    mocks.pathExists.mockImplementationOnce(
      () => new Promise(resolve => (finishValidation = resolve))
    )
    fireEvent.click(screen.getByRole('button', { name: 'Clone' }))
    await waitFor(() => expect(finishValidation).toBeDefined())
    expect(screen.getByRole('button', { name: 'Close' })).toHaveProperty(
      'disabled',
      true
    )
    finishValidation(false)
    await waitFor(() => expect(mocks.clone).toHaveBeenCalledOnce())
    expect(screen.getByRole('button', { name: 'Close' })).toHaveProperty(
      'disabled',
      true
    )
    expect(mocks.clone).toHaveBeenCalledWith(
      'https://github.com/alice/project.git',
      '/clones/final',
      { host: 'github.com', login: 'alice', id: 12 },
      undefined,
      expect.any(Function)
    )
    expect(mocks.readdir).not.toHaveBeenCalled()
    finishClone({ path: '/clones/final', staging: '/clones/.stage' })
    expect(await screen.findByRole('status')).toHaveProperty(
      'textContent',
      expect.stringContaining('Cloned /clones/final')
    )
    expect(screen.getByRole('button', { name: 'Close' })).not.toHaveProperty(
      'disabled',
      true
    )
  })

  it('does not start a clone if the dialog unmounts during preparation', async () => {
    const account = new Account(
      'alice',
      'https://api.github.com',
      'token',
      [],
      '',
      12,
      ''
    )
    const { unmount } = render(
      <CloneRepository
        dispatcher={
          { closeFoldout: vi.fn(), clone: vi.fn() } as unknown as Dispatcher
        }
        onDismissed={vi.fn()}
        accounts={[account]}
        initialURL="https://github.com/alice/project.git"
        selectedTab={CloneRepositoryTab.GitHub}
        selectedAccount={account}
        onTabSelected={vi.fn()}
        apiRepositories={new Map()}
        onRefreshRepositories={vi.fn()}
        isTopMost={true}
      />
    )
    const path = await screen.findByRole('textbox', { name: 'Local path' })
    await waitFor(() => expect(path).toHaveProperty('value', '/clones/project'))
    fireEvent.click(
      screen.getByRole('checkbox', { name: /Clone with the selected account/ })
    )
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Clone' })).not.toHaveProperty(
        'disabled',
        true
      )
    )
    let finishValidation!: (exists: boolean) => void
    mocks.pathExists.mockImplementationOnce(
      () => new Promise(resolve => (finishValidation = resolve))
    )
    fireEvent.click(screen.getByRole('button', { name: 'Clone' }))
    await waitFor(() => expect(finishValidation).toBeDefined())
    unmount()
    await act(async () => finishValidation(false))
    expect(mocks.clone).not.toHaveBeenCalled()
  })
})
