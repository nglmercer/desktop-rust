import { describe, it } from 'vitest'
import assert from 'node:assert'
import * as path from 'path'

import { Repository } from '../../../src/models/repository'
import { reset, GitResetMode } from '../../../src/lib/git/reset'
import {
  setupEmptyRepository,
  setupFixtureRepository,
} from '../../helpers/repositories'
import { exec } from 'dugite'
import { getStatusOrThrow } from '../../helpers/status'

import { readFile, writeFile } from 'fs/promises'

describe('git/reset', () => {
  describe('reset', () => {
    it('can hard reset a repository', async t => {
      const testRepoPath = await setupFixtureRepository(t, 'test-repo')
      const repository = new Repository(testRepoPath, -1, null, false)

      const repoPath = repository.path
      const fileName = 'README.md'
      const filePath = path.join(repoPath, fileName)

      await writeFile(filePath, 'Hi world\n')

      await reset(repository, GitResetMode.Hard, 'HEAD')

      const status = await getStatusOrThrow(repository)
      assert.equal(status.workingDirectory.files.length, 0)
    })

    it('can soft reset a repository to its parent', async t => {
      const repository = await setupEmptyRepository(t)
      const filePath = path.join(repository.path, 'file.txt')
      await writeFile(filePath, 'one\n')
      let result = await exec(['add', 'file.txt'], repository.path)
      assert.strictEqual(result.exitCode, 0, result.stderr)
      result = await exec(['commit', '-m', 'one'], repository.path)
      assert.strictEqual(result.exitCode, 0, result.stderr)
      const first = await exec(['rev-parse', 'HEAD'], repository.path)
      assert.strictEqual(first.exitCode, 0, first.stderr)
      await writeFile(filePath, 'two\n')
      result = await exec(['commit', '-am', 'two'], repository.path)
      assert.strictEqual(result.exitCode, 0, result.stderr)

      await reset(repository, GitResetMode.Soft, 'HEAD~1')

      const head = await exec(['rev-parse', 'HEAD'], repository.path)
      assert.strictEqual(head.stdout.trim(), first.stdout.trim())
      const parent = await exec(['rev-parse', 'HEAD^'], repository.path)
      assert.strictEqual(parent.exitCode, 128)
      const staged = await exec(
        ['diff', '--cached', '--name-only'],
        repository.path
      )
      assert.strictEqual(staged.stdout, 'file.txt\n')
      assert.strictEqual(
        await readFile(filePath, 'utf8'),
        'two\n',
        'soft reset preserves working tree content'
      )
      assert.strictEqual(head.exitCode, 0)
    })
  })
})
