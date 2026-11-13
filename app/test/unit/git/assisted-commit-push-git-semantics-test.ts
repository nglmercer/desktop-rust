import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { exec } from 'dugite'
import { describe, it } from 'vitest'
import { Repository } from '../../../src/models/repository'
import { readAssistedCommitPushDestination } from '../../../src/lib/git/assisted-commit-push'
import { getAssistedCommitPushConfigParameters } from '../../../src/lib/git/push'
import { isolateGitConfig } from '../../helpers/git-config'

describe('assisted push Git configuration semantics', () => {
  it('refuses multiple destinations before a reset can expose them to Git', async t => {
    await isolateGitConfig(t)
    const root = await mkdtemp(join(tmpdir(), 'desktop-push-reset-'))
    const repository = join(root, 'repository')
    const intended = join(root, 'intended.git')
    const unintended = join(root, 'unintended.git')
    const previous = process.env.GIT_CONFIG_PARAMETERS
    try {
      // The production helper preserves inherited parameters; isolate them for
      // this regression test so the only destinations are those below.
      delete process.env.GIT_CONFIG_PARAMETERS
      await mkdir(repository)
      const env = {
        HOME: root,
        GIT_CONFIG_GLOBAL: join(root, 'absent-global'),
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_PARAMETERS: '',
      }
      const run = (args: string[], overrides: Record<string, string> = {}) =>
        exec(args, repository, { env: { ...env, ...overrides } })
      for (const args of [
        ['init', '-q', '-b', 'main'],
        ['init', '--bare', '-q', intended],
        ['init', '--bare', '-q', unintended],
        [
          '-c',
          'user.name=Test',
          '-c',
          'user.email=test@example.invalid',
          '-c',
          'commit.gpgsign=false',
          'commit',
          '--allow-empty',
          '-qm',
          'initial',
        ],
        ['remote', 'add', 'origin', intended],
        ['config', '--add', 'remote.origin.pushurl', intended],
        ['config', '--add', 'remote.origin.pushurl', unintended],
      ]) {
        const result = await run(args)
        assert.equal(result.exitCode, 0, result.stderr)
      }
      const destination = await readAssistedCommitPushDestination(
        new Repository(repository, -1, null, false),
        'refs/heads/main'
      )
      assert.equal(destination.kind, 'unavailable')
      if (destination.kind === 'unavailable') {
        assert.match(
          destination.error.message,
          /one configured push destination/
        )
      }
      const pinned = {
        GIT_CONFIG_PARAMETERS: getAssistedCommitPushConfigParameters(
          'origin',
          intended,
          'push-url'
        ),
      }
      const urls = await run(
        ['remote', 'get-url', '--push', '--all', '--', 'origin'],
        pinned
      )
      assert.equal(urls.exitCode, 0, urls.stderr)
      const isSingleIntendedDestination = urls.stdout === `${intended}\n`
      if (!isSingleIntendedDestination) {
        assert.ok(urls.stdout.split('\n').includes(unintended))
      }

      // This runs only against temporary local repositories. Git versions
      // that do not reset multi-valued URLs can update the unintended remote
      // before reporting an error, so the caller must refuse such a list.
      const push = await run(
        ['push', '--', 'origin', 'HEAD:refs/heads/main'],
        pinned
      )
      const exists = async (path: string) =>
        (
          await run([
            '--git-dir',
            path,
            'rev-parse',
            '--verify',
            'refs/heads/main',
          ])
        ).exitCode === 0
      assert.equal(await exists(intended), true, push.stderr)
      assert.equal(await exists(unintended), !isSingleIntendedDestination)
    } finally {
      if (previous === undefined) delete process.env.GIT_CONFIG_PARAMETERS
      else process.env.GIT_CONFIG_PARAMETERS = previous
      await rm(root, { recursive: true, force: true })
    }
  })
})
