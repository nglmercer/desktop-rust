// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  existsSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { afterAll, afterEach, beforeAll, describe, it } from 'vitest'
import {
  type AccountRepo,
  BINARY,
  type Json,
  clean,
  object,
  privateFile,
} from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

const ACTION = { type: 'checkout_pull_request', number: 7 }
describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1')(
  'pull request checkout tracking',
  () => {
    let suite: Credentials
    beforeAll(async () => {
      suite = new Credentials()
      await suite.start()
    }, 150_000)
    afterAll(async () => {
      if (suite) await suite.close()
    })
    afterEach(async () => new Promise<void>(resolve => setImmediate(resolve)))

    const identity = (owner: string, identifier = 9003): Json => ({
      id: identifier,
      name: 'repo',
      full_name: `${owner}/repo`,
      html_url: `https://${suite.data.host}/${owner}/repo`,
      owner: { login: owner },
      default_branch: 'main',
      fork: false,
      parent: null,
    })
    function prepare(
      name: string,
      options: {
        headOwner?: string
        branch?: string
        declared?: boolean
        deleted?: boolean
        editable?: boolean
      } = {}
    ): { repo: AccountRepo; oid: string } {
      const {
        headOwner = 'fork',
        branch = 'tracking-topic',
        declared = true,
        deleted = false,
        editable = false,
      } = options
      suite.configure({ host: suite.service.host })
      const repo = suite.seed(name)
      const base: Json = {
        ...object(object(suite.data.repositories).alicecase),
        default_branch: 'main',
      }
      const head = deleted
        ? null
        : identity(headOwner, headOwner === 'fork' ? Number(base.id) : 9003)
      repo.git('switch', '-qc', 'fixture-pr-source')
      repo.write('file', 'verified tracking pull request\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'tracking PR')
      const oid = repo.git('rev-parse', 'HEAD').toString().trim()
      repo.git('switch', '-q', 'main')
      const project = path.join(
        suite.service.projects,
        declared ? `${headOwner}/repo.git` : 'fork/repo.git'
      )
      assert.ok(existsSync(project))
      repo.git(
        '-c',
        'core.hooksPath=/dev/null',
        'push',
        '-q',
        project,
        `+${oid}:${declared ? `refs/heads/${branch}` : 'refs/pull/7/head'}`
      )
      if (declared)
        suite.service.update({
          allowed: {
            ...suite.service.control.allowed,
            [`/${headOwner}/repo.git`]: 'alicecase',
          },
        })
      const pull = {
        number: 7,
        base: { repo: base },
        maintainer_can_modify: editable,
        head: { ref: branch, sha: oid, repo: head, user: { login: headOwner } },
      }
      const responses =
        head === null || headOwner === 'fork'
          ? {}
          : { [`/repos/${headOwner}/repo`]: head }
      suite.update({
        pull_requests: { '/repos/fork/repo/pulls/7': pull },
        repository_responses: responses,
      })
      suite.assign(repo)
      suite.clear()
      suite.clearTransport()
      return { repo, oid }
    }
    function setting(
      repo: AccountRepo,
      branch: string,
      key: string
    ): string | null {
      const result = spawnSync(
        suite.realGit,
        ['-C', repo.root, 'config', '--get', `branch.${branch}.${key}`],
        { env: suite.env, encoding: 'utf8' }
      )
      assert.ok(result.status === 0 || result.status === 1)
      return result.status === 0 ? result.stdout.replace(/\n$/, '') : null
    }
    function finished(repo: AccountRepo): void {
      assert.equal(
        repo.git('for-each-ref', 'refs/desktop/pr-checkout/').toString(),
        ''
      )
      const git = path.join(repo.root, '.git')
      assert.ok(
        !existsSync(path.join(git, 'config.lock')) &&
          !existsSync(path.join(git, 'config.lock.lock'))
      )
      const temporary = path.join(git, 'desktop-pr-checkout')
      assert.ok(
        !existsSync(temporary) ||
          !readdirSync(temporary).some(name => name.endsWith('.create-hooks'))
      )
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
      suite.assertPrivateStorage(repo)
    }
    it('tracks existing origin and fork remotes with native branch configuration', () => {
      const branch = 'native-tracking'
      let { repo, oid } = prepare('tracking-known-origin', { branch })
      repo.git('config', 'branch.autoSetupMerge', 'false')
      repo.git('config', 'branch.autoSetupRebase', 'remote')
      repo.git('config', 'remote.pushDefault', 'upstream')
      repo.git('config', `branch.${branch}.gh-merge-base`, 'caller-base')
      assert.equal(suite.github(repo, ACTION).code, 0)
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        `${branch}\n`
      )
      assert.equal(
        repo
          .git('rev-parse', `refs/remotes/origin/${branch}`)
          .toString()
          .trim(),
        oid
      )
      assert.equal(setting(repo, branch, 'remote'), 'origin')
      assert.equal(setting(repo, branch, 'merge'), `refs/heads/${branch}`)
      assert.equal(setting(repo, branch, 'rebase'), 'true')
      assert.equal(setting(repo, branch, 'pushRemote'), null)
      assert.equal(setting(repo, branch, 'gh-merge-base'), 'caller-base')
      assert.equal(
        repo.git('config', 'remote.pushDefault').toString(),
        'upstream\n'
      )
      assert.ok(
        suite.service
          .records()
          .filter(row => row.actor !== null)
          .every(row => String(row.path).startsWith('/fork/repo.git/'))
      )
      suite.assertActor()
      finished(repo)
      ;({ repo, oid } = prepare('tracking-known-fork', {
        headOwner: 'parent',
        branch: 'main',
      }))
      repo.git('branch', '-m', 'main', 'fixture-base')
      assert.equal(suite.github(repo, ACTION).code, 0)
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        'main\n'
      )
      assert.equal(
        repo.git('rev-parse', 'refs/remotes/upstream/main').toString().trim(),
        oid
      )
      assert.equal(setting(repo, 'main', 'remote'), 'upstream')
      assert.equal(setting(repo, 'main', 'merge'), 'refs/heads/main')
      assert.equal(setting(repo, 'main', 'pushRemote'), null)
      assert.ok(
        suite.service
          .records()
          .filter(row => row.actor !== null)
          .every(row => String(row.path).startsWith('/parent/repo.git/'))
      )
      suite.assertActor()
      finished(repo)
    }, 120_000)
    it('uses the tracked fork transport for LFS smudge on a repeated fast-forward', () => {
      const { repo, oid } = prepare('tracking-fork-lfs', {
        headOwner: 'parent',
        branch: 'main',
      })
      repo.git('branch', '-m', 'main', 'fixture-base')
      assert.equal(suite.github(repo, ACTION).code, 0)
      repo.git('lfs', 'install', '--local')
      const payload = Buffer.from('known fork PR fast-forward LFS body\n')
      const hash = createHash('sha256').update(payload).digest('hex')
      writeFileSync(path.join(suite.service.objects, hash), payload)
      const pointer = Buffer.from(
        `version https://git-lfs.github.com/spec/v1\noid sha256:${hash}\nsize ${payload.length}\n`
      )
      const remote = path.join(suite.service.projects, 'parent/repo.git')
      const env = {
        ...process.env,
        GIT_INDEX_FILE: path.join(suite.root, 'tracking-remote-lfs.index'),
        GIT_AUTHOR_NAME: 'Fixture Writer',
        GIT_AUTHOR_EMAIL: 'writer@example.invalid',
        GIT_COMMITTER_NAME: 'Fixture Writer',
        GIT_COMMITTER_EMAIL: 'writer@example.invalid',
      }
      const remoteGit = (args: string[], input?: Buffer): string => {
        const result = spawnSync(
          suite.realGit,
          ['--git-dir', remote, ...args],
          { input, env, encoding: 'buffer', timeout: 10_000 }
        )
        assert.equal(result.status, 0, result.stderr.toString())
        return result.stdout.toString().trim()
      }
      remoteGit(['read-tree', 'refs/heads/main'])
      for (const [file, body] of [
        [
          '.gitattributes',
          Buffer.from('*.asset filter=lfs diff=lfs merge=lfs -text\n'),
        ],
        ['fork.asset', pointer],
      ] as const) {
        const blob = remoteGit(['hash-object', '-w', '--stdin'], body)
        remoteGit([
          'update-index',
          '--add',
          '--cacheinfo',
          `100644,${blob},${file}`,
        ])
      }
      const tree = remoteGit(['write-tree'])
      const next = remoteGit([
        'commit-tree',
        tree,
        '-p',
        oid,
        '-m',
        'fork LFS fast forward',
      ])
      remoteGit(['update-ref', 'refs/heads/main', next])
      const pull = object(
        object(suite.data.pull_requests)['/repos/fork/repo/pulls/7']
      )
      const head = object(pull.head)
      head.sha = next
      pull.head = head
      suite.update({ pull_requests: { '/repos/fork/repo/pulls/7': pull } })
      suite.clearTransport()
      const media = repo
        .git('lfs', 'env')
        .toString()
        .split('\n')
        .find(line => line.startsWith('LocalMediaDir='))
        ?.slice('LocalMediaDir='.length)
      assert.ok(media)
      const cached = path.join(media, hash.slice(0, 2), hash.slice(2, 4), hash)
      assert.ok(!existsSync(cached))
      const command = spawnSync('sh', ['-c', 'command -v git-lfs'], {
        env: suite.env,
        encoding: 'utf8',
      })
      const realLfs = process.env.GIT_LFS_BIN ?? command.stdout.trim()
      assert.ok(realLfs, 'Install git-lfs or set GIT_LFS_BIN')
      const log = path.join(suite.root, 'tracking-lfs-route.jsonl')
      const wrapper = path.join(suite.root, 'bin', 'git-lfs')
      assert.ok(!existsSync(wrapper))
      writeFileSync(
        wrapper,
        '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_LFS_FIXTURE" "$@"\n',
        { mode: 0o700 }
      )
      Object.assign(suite.env, {
        CREDENTIALS_LFS_FIXTURE: path.resolve(
          'rust/tests/fixtures/lfs-route.ts'
        ),
        CREDENTIALS_LFS_ROUTE_LOG: log,
        CREDENTIALS_REAL_LFS: realLfs,
      })
      try {
        assert.equal(suite.github(repo, ACTION).code, 0)
      } finally {
        rmSync(wrapper, { force: true })
      }
      assert.deepEqual(
        readFileSync(path.join(repo.root, 'fork.asset')),
        payload
      )
      assert.deepEqual(readFileSync(cached), payload)
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), next)
      assert.ok(
        suite.service
          .records()
          .some(
            row => row.actor !== null && String(row.path).includes('/info/lfs/')
          )
      )
      suite.assertActor()
      finished(repo)
    }, 120_000)

    it('preserves existing branch tracking configuration across repeated checkouts', () => {
      const branch = 'existing-tracking'
      const { repo, oid } = prepare('tracking-existing', { branch })
      repo.git('branch', branch, 'main')
      for (const [key, value] of [
        ['remote', 'caller-remote'],
        ['pushRemote', 'caller-push'],
        ['merge', 'refs/heads/caller'],
        ['rebase', 'interactive'],
        ['gh-merge-base', 'caller-base'],
      ])
        repo.git('config', `branch.${branch}.${key}`, value)
      const configured = readFileSync(path.join(repo.root, '.git', 'config'))
      for (let i = 0; i < 2; i++) {
        suite.clearTransport()
        assert.equal(suite.github(repo, ACTION).code, 0)
        assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), oid)
        assert.deepEqual(
          readFileSync(path.join(repo.root, '.git', 'config')),
          configured
        )
        assert.equal(
          repo
            .git('rev-parse', `refs/remotes/origin/${branch}`)
            .toString()
            .trim(),
          oid
        )
        suite.assertActor()
        finished(repo)
      }
    }, 90_000)
    it('chooses tracking destinations for missing, editable, deleted, and mixed-case forks', () => {
      for (const [name, deleted, editable, branch, expected] of [
        ['uneditable', false, false, 'uneditable-topic', 'uneditable-topic'],
        ['editable', false, true, 'editable-topic', 'editable-topic'],
        ['deleted', true, true, 'main', 'contributor/main'],
      ] as const) {
        const { repo, oid } = prepare(`tracking-missing-${name}`, {
          headOwner: 'contributor',
          branch,
          declared: false,
          deleted,
          editable,
        })
        const remotes = repo.git('for-each-ref', 'refs/remotes/')
        assert.equal(suite.github(repo, ACTION).code, 0)
        assert.equal(
          repo.git('symbolic-ref', '--short', 'HEAD').toString(),
          `${expected}\n`
        )
        assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), oid)
        const remote =
          editable && !deleted
            ? `https://${suite.data.host}/contributor/repo.git`
            : 'origin'
        assert.equal(setting(repo, expected, 'remote'), remote)
        assert.equal(setting(repo, expected, 'pushRemote'), remote)
        assert.equal(
          setting(repo, expected, 'merge'),
          editable && !deleted ? `refs/heads/${branch}` : 'refs/pull/7/head'
        )
        assert.deepEqual(repo.git('for-each-ref', 'refs/remotes/'), remotes)
        if (editable && !deleted)
          assert.ok(
            suite
              .calls()
              .some(
                row =>
                  Array.isArray(row.args) &&
                  row.args.slice(0, 3).join(' ') ===
                    'config get git_protocol' &&
                  row.tokens_absent
              )
          )
        suite.assertActor()
        finished(repo)
      }
      const branch = 'editable-case-topic'
      const { repo } = prepare('tracking-missing-editable-case', {
        headOwner: 'ContributorCase',
        branch,
        declared: false,
        editable: true,
      })
      const pull = object(
        object(suite.data.pull_requests)['/repos/fork/repo/pulls/7']
      )
      const head = object(pull.head)
      const headRepo = object(head.repo)
      Object.assign(headRepo, {
        name: 'RepoCase',
        full_name: 'ContributorCase/RepoCase',
        html_url: `https://${suite.data.host}/ContributorCase/RepoCase`,
      })
      head.repo = headRepo
      pull.head = head
      suite.update({ pull_requests: { '/repos/fork/repo/pulls/7': pull } })
      assert.equal(suite.github(repo, ACTION).code, 0)
      const expected = `https://${suite.data.host}/ContributorCase/RepoCase.git`
      assert.equal(setting(repo, branch, 'remote'), expected)
      assert.equal(setting(repo, branch, 'pushRemote'), expected)
      suite.assertActor()
      finished(repo)
    }, 120_000)
    it('uses the preferred SSH URL for editable forks without leaking credentials', () => {
      const authority = suite.service.host
      suite.service.host = 'github.com'
      try {
        const branch = 'ssh-editable-topic'
        const { repo, oid } = prepare('tracking-ssh-preference', {
          headOwner: 'contributor',
          branch,
          declared: false,
          editable: true,
        })
        repo.git(
          'remote',
          'set-url',
          'origin',
          'ssh://git@github.com/fork/repo.git'
        )
        suite.update({ git_protocol: 'ssh' })
        const ssh = path.join(suite.root, 'bin', 'checkout-tracking-ssh')
        const log = path.join(suite.root, 'checkout-tracking-ssh.jsonl')
        writeFileSync(
          ssh,
          '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_SSH_FIXTURE" "$@"\n',
          { mode: 0o700 }
        )
        Object.assign(suite.env, {
          GIT_SSH: ssh,
          GIT_SSH_VARIANT: 'ssh',
          CREDENTIALS_SSH_FIXTURE: path.resolve(
            'rust/tests/fixtures/native-ssh.ts'
          ),
          CREDENTIALS_SSH_LOG: log,
          CREDENTIALS_SSH_REPO: path.join(
            suite.service.projects,
            'fork/repo.git'
          ),
        })
        try {
          suite.clearTransport()
          assert.equal(suite.github(repo, ACTION).code, 0)
        } finally {
          delete suite.env.GIT_SSH
          delete suite.env.GIT_SSH_VARIANT
        }
        assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), oid)
        assert.equal(
          setting(repo, branch, 'remote'),
          'git@github.com:contributor/repo.git'
        )
        assert.equal(
          setting(repo, branch, 'pushRemote'),
          'git@github.com:contributor/repo.git'
        )
        assert.equal(setting(repo, branch, 'merge'), `refs/heads/${branch}`)
        const rows = suite.records(log)
        assert.ok(
          rows.length &&
            rows.every(row => row.argv_clean && row.alice_environment_clean)
        )
        assert.equal(suite.service.records().length, 0)
        assert.ok(!existsSync(suite.nativeLog))
        finished(repo)
      } finally {
        suite.service.host = authority
      }
    }, 90_000)

    it('preserves nonempty caller merge configuration and fills an empty one', () => {
      for (const empty of [false, true]) {
        const branch = empty ? 'empty-merge' : 'existing-merge'
        const { repo } = prepare(`tracking-${branch}`, {
          headOwner: 'contributor',
          branch,
          declared: false,
        })
        for (const [key, value] of [
          ['remote', 'caller-remote'],
          ['pushRemote', 'caller-push'],
          ['merge', empty ? '' : 'refs/heads/caller'],
          ['gh-merge-base', 'caller-base'],
          ['rebase', 'interactive'],
        ])
          repo.git('config', `branch.${branch}.${key}`, value)
        const configured = readFileSync(path.join(repo.root, '.git', 'config'))
        assert.equal(suite.github(repo, ACTION).code, 0)
        assert.equal(setting(repo, branch, 'gh-merge-base'), 'caller-base')
        assert.equal(setting(repo, branch, 'rebase'), 'interactive')
        if (empty) {
          assert.equal(setting(repo, branch, 'remote'), 'origin')
          assert.equal(setting(repo, branch, 'pushRemote'), 'origin')
          assert.equal(setting(repo, branch, 'merge'), 'refs/pull/7/head')
        } else
          assert.deepEqual(
            readFileSync(path.join(repo.root, '.git', 'config')),
            configured
          )
        suite.assertActor()
        finished(repo)
      }
    }, 90_000)

    it('preserves caller changes when canonical remote refs race checkout', async () => {
      for (const race of ['symbolic', 'tip']) {
        const branch = `raced-${race}`
        const { repo } = prepare(`tracking-${branch}`, { branch })
        const old = repo.git('rev-parse', 'main').toString().trim()
        const reference = `refs/remotes/origin/${branch}`
        repo.git('update-ref', reference, old)
        repo.git('branch', 'protected', old)
        rmSync(suite.gateStarted, { force: true })
        rmSync(suite.gateRelease, { force: true })
        privateFile(suite.control, JSON.stringify({ gate: 'update-ref' }))
        const process = spawn(
          BINARY,
          ['--config', suite.registry, '-C', repo.root, 'repo', '-'],
          { env: suite.env }
        )
        let stdout = ''
        let stderr = ''
        process.stdout.on('data', chunk => {
          stdout += chunk.toString()
        })
        process.stderr.on('data', chunk => {
          stderr += chunk.toString()
        })
        const exited = new Promise<number | null>((resolve, reject) => {
          process.once('error', reject)
          process.once('exit', resolve)
        })
        process.stdin.end(
          JSON.stringify({ method: 'github', request: { action: ACTION } })
        )
        try {
          const deadline = Date.now() + 25_000
          while (!existsSync(suite.gateStarted) && Date.now() < deadline)
            await new Promise(resolve => setTimeout(resolve, 20))
          assert.ok(
            existsSync(suite.gateStarted),
            'Canonical remote ref update did not reach gate'
          )
          let foreign = ''
          if (race === 'symbolic')
            repo.git('symbolic-ref', reference, 'refs/heads/protected')
          else {
            foreign = repo
              .git(
                'commit-tree',
                `${old}^{tree}`,
                '-p',
                old,
                '-m',
                'caller remote tip'
              )
              .toString()
              .trim()
            repo.git('update-ref', reference, foreign)
          }
          writeFileSync(suite.gateRelease, 'release')
          const code = await Promise.race([
            exited,
            new Promise<never>((_, reject) => {
              const timer = setTimeout(
                () => reject(new Error('Checkout timed out')),
                60_000
              )
              timer.unref()
            }),
          ])
          clean(stdout)
          clean(stderr)
          assert.notEqual(code, 0)
          assert.equal(
            repo.git('symbolic-ref', '--short', 'HEAD').toString(),
            'main\n'
          )
          assert.equal(
            repo.git('rev-parse', 'protected').toString().trim(),
            old
          )
          const show = spawnSync(
            suite.realGit,
            [
              '-C',
              repo.root,
              'show-ref',
              '--verify',
              '--quiet',
              `refs/heads/${branch}`,
            ],
            { env: suite.env }
          )
          assert.equal(show.status, 1)
          if (race === 'symbolic')
            assert.equal(
              repo.git('symbolic-ref', reference).toString(),
              'refs/heads/protected\n'
            )
          else
            assert.equal(
              repo.git('rev-parse', reference).toString().trim(),
              foreign
            )
          suite.assertActor()
          finished(repo)
        } finally {
          writeFileSync(suite.gateRelease, 'release')
          privateFile(suite.control, '{}')
          if (process.exitCode === null && process.signalCode === null)
            process.kill('SIGKILL')
        }
      }
    }, 120_000)

    it('protects concurrent branch configuration changes during checkout and fetch', async () => {
      let branch = 'config-after-checkout'
      let { repo, oid } = prepare('tracking-config-hook', {
        headOwner: 'contributor',
        branch,
        declared: false,
      })
      const hook = path.join(repo.root, '.git', 'hooks', 'post-checkout')
      writeFileSync(
        hook,
        `#!/bin/sh\ngit config branch.${branch}.merge refs/heads/caller-change\n`,
        { mode: 0o700 }
      )
      suite.github(repo, ACTION, null, true)
      assert.equal(
        repo.git('symbolic-ref', '--short', 'HEAD').toString(),
        `${branch}\n`
      )
      assert.equal(repo.git('rev-parse', 'HEAD').toString().trim(), oid)
      assert.equal(setting(repo, branch, 'merge'), 'refs/heads/caller-change')
      assert.equal(setting(repo, branch, 'remote'), null)
      assert.equal(setting(repo, branch, 'pushRemote'), null)
      suite.assertActor()
      finished(repo)
      branch = 'config-during-fetch'
      ;({ repo, oid } = prepare('tracking-config-fetch', {
        headOwner: 'contributor',
        branch,
        declared: false,
      }))
      const before = suite.evidence(repo, false)
      suite.service.update({ blockPath: '/info/refs' })
      const child = spawn(
        BINARY,
        ['--config', suite.registry, '-C', repo.root, 'repo', '-'],
        { env: suite.env }
      )
      let stdout = ''
      let stderr = ''
      child.stdout.on('data', chunk => {
        stdout += chunk.toString()
      })
      child.stderr.on('data', chunk => {
        stderr += chunk.toString()
      })
      const exited = new Promise<number | null>((resolve, reject) => {
        child.once('error', reject)
        child.once('exit', resolve)
      })
      child.stdin.end(
        JSON.stringify({ method: 'github', request: { action: ACTION } })
      )
      try {
        const deadline = Date.now() + 25_000
        while (!existsSync(suite.service.started) && Date.now() < deadline)
          await new Promise(resolve => setTimeout(resolve, 20))
        assert.ok(
          existsSync(suite.service.started),
          'PR fetch did not reach HTTP gate'
        )
        repo.git('config', `branch.${branch}.merge`, 'refs/heads/concurrent')
        writeFileSync(suite.service.release, 'release')
        const code = await Promise.race([
          exited,
          new Promise<never>((_, reject) => {
            const timer = setTimeout(
              () => reject(new Error('Checkout timed out')),
              60_000
            )
            timer.unref()
          }),
        ])
        clean(stdout)
        clean(stderr)
        assert.notEqual(code, 0)
        assert.deepEqual(suite.evidence(repo, false), before)
        assert.equal(setting(repo, branch, 'merge'), 'refs/heads/concurrent')
        assert.equal(setting(repo, branch, 'remote'), null)
        suite.assertActor()
        finished(repo)
      } finally {
        suite.service.update({ blockPath: undefined })
        writeFileSync(suite.service.release, 'release')
        if (child.exitCode === null && child.signalCode === null)
          child.kill('SIGKILL')
      }
    }, 120_000)
  }
)
