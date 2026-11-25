import { describe, it } from 'vitest'
import assert from 'node:assert'
import * as Path from 'path'

import { Repository } from '../../../src/models/repository'
import { git } from '../../../src/lib/git'

import { setupFixtureRepository } from '../../helpers/repositories'
import { mkdir, readFile } from 'fs/promises'
import { createTempDirectory } from '../../helpers/temp'
import { setupOwnershipCheck } from '../../helpers/ownership-check'
import { addSafeDirectory } from '../../../src/lib/git/config'

describe('git/config', () => {
  describe('addSafeDirectory', () => {
    for (const value of [
      '*',
      '/*',
      '/shared/*',
      '~/repository',
      '%(prefix)/repository',
      'relative',
    ]) {
      it(`rejects non-exact directory ${JSON.stringify(value)}`, async t => {
        const configPath = await setupOwnershipCheck(t)
        const before = await readFile(configPath, 'utf8')

        await assert.rejects(addSafeDirectory(value))

        assert.strictEqual(await readFile(configPath, 'utf8'), before)
      })
    }

    it.skipIf(__WIN32__)(
      'rejects an existing directory whose name denotes a subtree',
      async t => {
        const parent = await createTempDirectory(t)
        const directory = Path.join(parent, '*')
        await mkdir(directory)
        const configPath = await setupOwnershipCheck(t)
        const before = await readFile(configPath, 'utf8')

        await assert.rejects(addSafeDirectory(`${directory}/`))

        assert.strictEqual(await readFile(configPath, 'utf8'), before)
      }
    )
  })

  describe('GIT_CONFIG_PARAMETERS', () => {
    it('picks them up', async t => {
      const testRepoPath = await setupFixtureRepository(t, 'test-repo')
      const repository = new Repository(testRepoPath, -1, null, false)

      const withoutEnvOutput = await git(
        ['config', 'desktop.test'],
        repository.path,
        '',
        { successExitCodes: new Set([1]) }
      ).then(x => x.stdout)

      assert.equal(
        withoutEnvOutput.length,
        0,
        'Expected withoutEnvOutput to be empty'
      )
      const withEnvOutput = await git(
        ['config', 'desktop.test'],
        repository.path,
        '',
        { env: { GIT_CONFIG_PARAMETERS: "'desktop.test=1'" } }
      ).then(x => x.stdout)

      assert.equal(withEnvOutput, '1\n')
    })

    it('takes precedence over GIT_CONFIG_*', async t => {
      const testRepoPath = await setupFixtureRepository(t, 'test-repo')
      const repository = new Repository(testRepoPath, -1, null, false)

      const output = await git(['config', 'user.name'], repository.path, '', {
        env: {
          GIT_CONFIG_PARAMETERS: "'user.name=foobar'",
          GIT_CONFIG_COUNT: '1',
          GIT_CONFIG_KEY_0: 'user.name',
          GIT_CONFIG_VALUE_0: 'baz',
        },
      }).then(x => x.stdout)

      assert.equal(output, 'foobar\n')
    })
  })
})
