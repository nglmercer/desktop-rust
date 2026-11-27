import assert from 'node:assert'
import { beforeAll, beforeEach, describe, it, vi } from 'vitest'
import * as Path from 'path'
import * as React from 'react'

import type { RepositoryType } from '../../../src/lib/git/rev-parse'
import { Repository } from '../../../src/models/repository'
import type { Dispatcher } from '../../../src/ui/dispatcher'
import { fireEvent, render, screen, waitFor } from '../../helpers/ui/render'

const unsafePath = Path.resolve('repository with spaces')
const requestedPath = Path.join(unsafePath, 'subdirectory')
const repository = new Repository(requestedPath, 1, null, true)

const unsafeRepositoryType = async (): Promise<RepositoryType> => ({
  kind: 'unsafe',
  path: unsafePath,
})
const getRepositoryType = vi.fn(unsafeRepositoryType)
const addSafeDirectory = vi.fn(async (_path: string) => {})

beforeAll(() => {
  vi.doMock('../../../src/lib/git/index.ts', async () => ({
    ...(await vi.importActual('../../../src/lib/git/index.ts')),
    getRepositoryType,
    addSafeDirectory,
  }))
  vi.doMock('../../../src/ui/main-process-proxy.ts', async () => ({
    ...(await vi.importActual('../../../src/ui/main-process-proxy.ts')),
    sendDialogDidOpen: () => {},
    showOpenDialog: async () => null,
  }))
})

function createDispatcher() {
  const postError = vi.fn((_error: unknown) => {})
  const refreshRepository = vi.fn(async (_repository: Repository) => {})
  const addRepositories = vi.fn(async (_paths: ReadonlyArray<string>) => [])

  return {
    postError,
    refreshRepository,
    addRepositories,
    dispatcher: {
      postError,
      refreshRepository,
      addRepositories,
    } as unknown as Dispatcher,
  }
}

function returnRegularRepositoryAfterTrust() {
  let checked = false
  getRepositoryType.mockImplementation(async () => {
    if (!checked) {
      checked = true
      return unsafeRepositoryType()
    }
    return {
      kind: 'regular',
      topLevelWorkingDirectory: unsafePath,
      gitDir: Path.join(unsafePath, '.git'),
    }
  })
}

beforeEach(() => {
  getRepositoryType.mockReset()
  getRepositoryType.mockImplementation(unsafeRepositoryType)
  addSafeDirectory.mockReset()
  addSafeDirectory.mockImplementation(async () => {})
})

describe('AddExistingRepository directory exceptions', () => {
  it('accepts a repository path enclosed in double quotes', async () => {
    getRepositoryType.mockImplementationOnce(async () => ({
      kind: 'regular',
      topLevelWorkingDirectory: requestedPath,
      gitDir: Path.join(requestedPath, '.git'),
    }))
    const dispatcher = createDispatcher()
    const onDismissed = vi.fn()
    const { AddExistingRepository } = await import(
      '../../../src/ui/add-repository/add-existing-repository'
    )

    render(
      <AddExistingRepository
        dispatcher={dispatcher.dispatcher}
        path={`"${requestedPath}"`}
        onDismissed={onDismissed}
      />
    )

    fireEvent.click(
      screen.getByRole('button', { name: /add repository/i, hidden: true })
    )

    await waitFor(() => {
      assert.deepStrictEqual(getRepositoryType.mock.calls[0], [requestedPath])
      assert.deepStrictEqual(dispatcher.addRepositories.mock.calls[0], [
        [requestedPath],
      ])
    })

    assert.strictEqual(onDismissed.mock.calls.length, 1)
  })

  async function renderUnsafeRepository() {
    const { AddExistingRepository } = await import(
      '../../../src/ui/add-repository/add-existing-repository'
    )
    const dispatcher = createDispatcher()
    const onDismissed = vi.fn()
    const view = render(
      <AddExistingRepository
        dispatcher={dispatcher.dispatcher}
        path={requestedPath}
        onDismissed={onDismissed}
      />
    )

    fireEvent.click(
      screen.getByRole('button', { name: /add repository/i, hidden: true })
    )
    const trustButton = await screen.findByRole('button', {
      name: 'add an exception for this directory',
      hidden: true,
    })
    assert.deepStrictEqual(getRepositoryType.mock.calls[0], [requestedPath])

    return { ...dispatcher, ...view, onDismissed, trustButton }
  }

  it('trusts the exact unsafe directory and revalidates the entered path', async () => {
    returnRegularRepositoryAfterTrust()
    const view = await renderUnsafeRepository()

    fireEvent.click(view.trustButton)

    assert.ok(view.container.querySelector('.dialog-header .spin'))
    assert.deepStrictEqual(addSafeDirectory.mock.calls, [[unsafePath]])
    assert.strictEqual(getRepositoryType.mock.calls.length, 1)

    await waitFor(() => {
      assert.ok(view.container.querySelector('.dialog-header .spin') === null)
      assert.ok(
        screen.queryByRole('button', {
          name: 'add an exception for this directory',
          hidden: true,
        }) === null
      )
    })

    assert.deepStrictEqual(getRepositoryType.mock.calls, [
      [requestedPath],
      [requestedPath],
    ])
    assert.strictEqual(view.postError.mock.calls.length, 0)
    assert.strictEqual(view.addRepositories.mock.calls.length, 0)
    assert.strictEqual(view.onDismissed.mock.calls.length, 0)
  })

  it('posts a failed exception write and stops loading without revalidating', async () => {
    const error = new Error('Unable to write repository exception')
    addSafeDirectory.mockImplementation(async () => {
      throw error
    })
    const view = await renderUnsafeRepository()

    fireEvent.click(view.trustButton)

    assert.ok(view.container.querySelector('.dialog-header .spin'))
    await waitFor(() => {
      assert.deepStrictEqual(view.postError.mock.calls, [[error]])
      assert.ok(view.container.querySelector('.dialog-header .spin') === null)
    })

    assert.deepStrictEqual(addSafeDirectory.mock.calls, [[unsafePath]])
    assert.strictEqual(getRepositoryType.mock.calls.length, 1)
    assert.ok(
      screen.getByRole('button', {
        name: 'add an exception for this directory',
        hidden: true,
      })
    )
    assert.strictEqual(view.addRepositories.mock.calls.length, 0)
    assert.strictEqual(view.onDismissed.mock.calls.length, 0)
  })
})

describe('MissingRepository directory exceptions', () => {
  async function renderUnsafeRepository() {
    const { MissingRepository } = await import(
      '../../../src/ui/missing-repository'
    )
    const dispatcher = createDispatcher()
    const view = render(
      <MissingRepository
        dispatcher={dispatcher.dispatcher}
        repository={repository}
      />
    )
    const trustButton = await screen.findByRole('button', {
      name: /trust repository/i,
    })
    assert.deepStrictEqual(getRepositoryType.mock.calls[0], [requestedPath])

    return { ...dispatcher, ...view, trustButton }
  }

  it('trusts the exact unsafe directory and refreshes after rechecking the repository', async () => {
    returnRegularRepositoryAfterTrust()
    const view = await renderUnsafeRepository()

    fireEvent.click(view.trustButton)

    assert.strictEqual(view.trustButton.getAttribute('aria-disabled'), 'true')
    assert.ok(view.trustButton.querySelector('.spin'))
    assert.strictEqual(getRepositoryType.mock.calls.length, 1)
    assert.strictEqual(view.refreshRepository.mock.calls.length, 0)

    await waitFor(() => {
      assert.strictEqual(view.trustButton.hasAttribute('aria-disabled'), false)
      assert.ok(view.trustButton.querySelector('.spin') === null)
      assert.deepStrictEqual(view.refreshRepository.mock.calls, [[repository]])
    })

    assert.deepStrictEqual(addSafeDirectory.mock.calls, [[unsafePath]])
    assert.deepStrictEqual(getRepositoryType.mock.calls, [
      [requestedPath],
      [requestedPath],
    ])
    assert.strictEqual(view.postError.mock.calls.length, 0)
  })

  it('posts a failed exception write and enables retry without rechecking or refreshing', async () => {
    const error = new Error('Unable to write repository exception')
    addSafeDirectory.mockImplementation(async () => {
      throw error
    })
    const view = await renderUnsafeRepository()

    fireEvent.click(view.trustButton)

    assert.strictEqual(view.trustButton.getAttribute('aria-disabled'), 'true')
    assert.ok(view.trustButton.querySelector('.spin'))
    await waitFor(() => {
      assert.deepStrictEqual(view.postError.mock.calls, [[error]])
      assert.strictEqual(view.trustButton.hasAttribute('aria-disabled'), false)
      assert.ok(view.trustButton.querySelector('.spin') === null)
    })

    assert.deepStrictEqual(addSafeDirectory.mock.calls, [[unsafePath]])
    assert.strictEqual(getRepositoryType.mock.calls.length, 1)
    assert.strictEqual(view.refreshRepository.mock.calls.length, 0)
  })
})
