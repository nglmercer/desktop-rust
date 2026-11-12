// @vitest-environment node
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { afterAll, beforeAll, describe, it } from 'vitest'
import { AccountBridge } from './fixtures/account-bridge'
import { type Json, object } from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1')(
  'authenticated pull request checkout',
  () => {
    let suite: Credentials
    beforeAll(async () => {
      suite = new Credentials()
      await suite.start()
    }, 150_000)
    afterAll(async () => {
      if (suite) await suite.close()
    })

    it('checks out verified refs without overwriting local work and supports a parent remote', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('native-pr-checkout')
      repo.git('switch', '-qc', 'fixture-pr-source')
      repo.write('file', 'verified remote pull request content\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'remote PR')
      const first = repo.git('rev-parse', 'HEAD').toString().trim()
      const remote = path.join(suite.service.projects, 'fork/repo.git')
      const publish = (oid: string): void => {
        repo.git(
          '-c',
          'core.hooksPath=/dev/null',
          'push',
          '-q',
          remote,
          `+${oid}:refs/pull/7/head`
        )
      }
      publish(first)
      repo.git('switch', '-q', 'main')
      const alice = object(object(suite.data.repositories).alicecase)
      const pull = {
        number: 7,
        base: { repo: { ...alice, default_branch: 'main' } },
        head: {
          ref: 'reviewed-pr',
          sha: first,
          repo: { owner: { login: 'contributor' } },
          user: { login: 'contributor' },
        },
      }
      const updatePull = (changes: Partial<typeof pull.head> = {}): void => {
        Object.assign(pull.head, changes)
        suite.update({ pull_requests: { '/repos/fork/repo/pulls/7': pull } })
      }
      updatePull()
      suite.assign(repo)
      const stored = readFileSync(suite.registry)
      suite.clear()
      suite.clearTransport()
      const action = { type: 'checkout_pull_request', number: 7 }
      const temporaryRefs = (): string =>
        repo.git('for-each-ref', 'refs/desktop/pr-checkout/').toString()
      assert.equal(suite.github(repo, action).code, 0)
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        'reviewed-pr\n'
      )
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), first)
      assert.equal(
        readFileSync(path.join(repo.root, 'file'), 'utf8'),
        'verified remote pull request content\n'
      )
      assert.equal(temporaryRefs(), '')
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.ok(
        !suite
          .calls()
          .some(
            row =>
              Array.isArray(row.args) &&
              row.args[0] === 'pr' &&
              row.args[1] === 'checkout'
          )
      )
      suite.assertActor()
      suite.assertPrivateStorage(repo)
      const second = repo
        .git(
          'commit-tree',
          'HEAD^{tree}',
          '-p',
          'HEAD',
          '-m',
          'remote PR fast forward'
        )
        .toString()
        .trim()
      publish(second)
      updatePull({ sha: second })
      suite.clearTransport()
      assert.equal(suite.github(repo, action).code, 0)
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), second)
      assert.equal(temporaryRefs(), '')
      suite.assertActor()
      repo.write('file', 'local PR work must survive\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'local PR work')
      const ahead = repo.git('rev-parse', 'HEAD')
      suite.clearTransport()
      assert.equal(suite.github(repo, action).code, 0)
      assert.deepEqual(repo.git('rev-parse', 'HEAD'), ahead)
      const divergent = repo
        .git(
          'commit-tree',
          `${first}^{tree}`,
          '-p',
          first,
          '-m',
          'diverged remote PR'
        )
        .toString()
        .trim()
      publish(divergent)
      updatePull({ sha: divergent })
      let before = suite.evidence(repo)
      suite.clearTransport()
      suite.github(repo, action, null, true)
      assert.deepEqual(suite.evidence(repo), before)
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.equal(temporaryRefs(), '')
      updatePull({ sha: 'a'.repeat(40) })
      before = suite.evidence(repo)
      suite.clearTransport()
      suite.github(repo, action, null, true)
      assert.deepEqual(suite.evidence(repo), before)
      assert.equal(temporaryRefs(), '')
      repo.git('switch', '-q', 'main')
      updatePull({ ref: 'main', sha: divergent })
      suite.clearTransport()
      assert.equal(suite.github(repo, action).code, 0)
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        'contributor/main\n'
      )
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), divergent)
      assert.equal(temporaryRefs(), '')
      suite.assertActor()
      suite.clearTransport()
      before = suite.evidence(repo)
      suite.github(repo, action, `${suite.service.host}/other/repo`, true)
      assert.ok(!suite.service.records().some(row => row.actor !== null))
      assert.deepEqual(suite.evidence(repo), before)
      assert.deepEqual(readFileSync(suite.registry), stored)
      suite.assertPrivateStorage(repo)
      const parent = {
        ...alice,
        id: 9002,
        full_name: 'parent/repo',
        html_url: `${suite.service.url}/parent/repo`,
        owner: { login: 'parent' },
        fork: false,
        parent: null,
        default_branch: 'main',
      }
      const parentPull = {
        number: 9,
        base: { repo: parent },
        head: {
          ref: 'parent-review',
          sha: divergent,
          repo: { owner: { login: 'contributor' } },
          user: { login: 'contributor' },
        },
      }
      repo.git(
        '-c',
        'core.hooksPath=/dev/null',
        'push',
        '-q',
        path.join(suite.service.projects, 'parent/repo.git'),
        `+${divergent}:refs/pull/9/head`
      )
      suite.service.update({
        allowed: {
          ...suite.service.control.allowed,
          '/parent/repo.git': 'alicecase',
        },
      })
      suite.update({
        repository_responses: { '/repos/parent/repo': parent },
        pull_requests: { '/repos/parent/repo/pulls/9': parentPull },
      })
      suite.clearTransport()
      assert.equal(
        suite.github(
          repo,
          { type: 'checkout_pull_request', number: 9 },
          `${suite.service.host}/parent/repo`
        ).code,
        0
      )
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        'parent-review\n'
      )
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), divergent)
      assert.ok(
        suite.service
          .records()
          .some(
            row =>
              row.actor !== null &&
              String(row.path).startsWith('/parent/repo.git/')
          )
      )
      assert.equal(temporaryRefs(), '')
      assert.deepEqual(readFileSync(suite.registry), stored)
      suite.assertActor()
      suite.assertPrivateStorage(repo)
    }, 150_000)

    it('uses the same account for fetch and pull-request checkout over WebSocket', async () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('bridge-credentials')
      repo.git('switch', '-qc', 'fixture-bridge-pr-source')
      repo.write('file', 'bridge assigned PR content\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'bridge PR')
      const oid = repo.git('rev-parse', 'HEAD').toString().trim()
      repo.git(
        '-c',
        'core.hooksPath=/dev/null',
        'push',
        '-q',
        path.join(suite.service.projects, 'fork/repo.git'),
        `+${oid}:refs/pull/11/head`
      )
      repo.git('switch', '-q', 'main')
      const alice = object(object(suite.data.repositories).alicecase)
      const pull: Json = {
        number: 11,
        base: { repo: { ...alice, default_branch: 'main' } },
        head: {
          ref: 'bridge-review',
          sha: oid,
          repo: { owner: { login: 'contributor' } },
          user: { login: 'contributor' },
        },
      }
      suite.update({ pull_requests: { '/repos/fork/repo/pulls/11': pull } })
      suite.assign(repo)
      const basis = suite.account(repo).basis
      const record = suite.record(repo)
      const bridge = await AccountBridge.start(suite, repo)
      const stored = readFileSync(suite.registry)
      try {
        suite.clearTransport()
        assert.equal((await bridge.call('fetch', { remote: 'origin' })).code, 0)
        suite.assertActor()
        suite.clearTransport()
        assert.equal(
          (
            await bridge.call('github', {
              request: {
                repository: `${suite.service.host}/fork/repo`,
                account_basis: basis,
                action: { type: 'checkout_pull_request', number: 11 },
              },
            })
          ).code,
          0
        )
        assert.equal(
          repo.git('symbolic-ref', '--short', 'HEAD').toString(),
          'bridge-review\n'
        )
        assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), oid)
        assert.equal(
          repo.git('for-each-ref', 'refs/desktop/pr-checkout/').toString(),
          ''
        )
        suite.assertActor()
      } finally {
        await bridge.close()
      }
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.deepEqual(suite.record(repo), record)
      suite.assertPrivateStorage(repo)
    }, 90_000)
  }
)
