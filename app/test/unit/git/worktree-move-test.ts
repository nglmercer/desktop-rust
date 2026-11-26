import assert from 'node:assert'
import * as Path from 'path'
import { realpath } from 'fs/promises'
import { describe, it, vi, type TestContext } from 'vitest'
import * as dugite from 'dugite'
import { pathExists } from '../../../src/lib/path-exists'
import { Repository } from '../../../src/models/repository'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../../src/lib/custom-integration', () => ({}))

import { moveWorktree } from '../../../src/lib/git/worktree'
import { setupEmptyRepository } from '../../helpers/repositories'
import { makeCommit } from '../../helpers/repository-scaffolding'
import { createTempDirectory } from '../../helpers/temp'

async function setupWorktree(t: TestContext) {
  const repo = await setupEmptyRepository(t, 'main')
  await makeCommit(repo, { entries: [{ path: 'README', contents: 'hello' }] })

  const mainPath = await realpath(repo.path)
  // All linked worktrees live under a second managed temporary directory so
  // cleanup still removes them if creation, movement, or an assertion fails.
  const worktreeDirectory = await realpath(await createTempDirectory(t))
  const worktreePath = Path.join(worktreeDirectory, 'wt-a')
  await dugite.exec(
    ['worktree', 'add', '-b', 'feature-a', '--', worktreePath],
    mainPath
  )

  return { repo, mainPath, worktreePath }
}

async function listedWorktreePaths(repositoryPath: string) {
  const { stdout } = await dugite.exec(
    ['worktree', 'list', '--porcelain'],
    repositoryPath
  )
  return stdout
    .split('\n')
    .filter(l => l.startsWith('worktree '))
    .map(l => Path.normalize(l.substring('worktree '.length)))
}

describe('git/worktree moveWorktree', () => {
  it('renames the linked worktree the repository points to', async t => {
    const { mainPath, worktreePath } = await setupWorktree(t)
    const newPath = Path.join(Path.dirname(worktreePath), 'wt-renamed')
    const repository = new Repository(worktreePath, -1, null, false)

    await moveWorktree(repository, worktreePath, newPath)

    assert.strictEqual(await pathExists(worktreePath), false)
    assert.strictEqual(await pathExists(newPath), true)
    assert.deepStrictEqual(await listedWorktreePaths(mainPath), [
      Path.normalize(mainPath),
      Path.normalize(newPath),
    ])
  })

  it('renames another linked worktree from the main worktree', async t => {
    const { repo, mainPath, worktreePath } = await setupWorktree(t)
    const newPath = Path.join(Path.dirname(worktreePath), 'wt-renamed')

    await moveWorktree(repo, worktreePath, newPath)

    assert.strictEqual(await pathExists(worktreePath), false)
    assert.strictEqual(await pathExists(newPath), true)
    assert.deepStrictEqual(await listedWorktreePaths(mainPath), [
      Path.normalize(mainPath),
      Path.normalize(newPath),
    ])
  })

  it('renames a linked worktree from a sibling linked worktree', async t => {
    const { mainPath, worktreePath } = await setupWorktree(t)
    const siblingPath = Path.join(Path.dirname(worktreePath), 'wt-b')
    await dugite.exec(
      ['worktree', 'add', '-b', 'feature-b', '--', siblingPath],
      mainPath
    )
    const newPath = Path.join(Path.dirname(worktreePath), 'wt-renamed')
    const repository = new Repository(siblingPath, -1, null, false)

    await moveWorktree(repository, worktreePath, newPath)

    assert.strictEqual(await pathExists(newPath), true)
    assert.strictEqual(await pathExists(worktreePath), false)
  })
})
