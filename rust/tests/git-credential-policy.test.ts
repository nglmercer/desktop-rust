// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { afterAll, beforeAll, describe, it } from 'vitest'
import {
  ALICE,
  BINARY,
  BOB,
  EFFECTIVE,
  NONE,
  OTHER,
  clean,
  object,
  privateFile,
} from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1')(
  'Git credential policy',
  () => {
    let suite: Credentials
    beforeAll(async () => {
      suite = new Credentials()
      await suite.start()
    }, 150_000)
    afterAll(async () => {
      if (suite) await suite.close()
    })
    it('scopes assigned credentials to the reviewed named fetch', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('reviewed-fetch-credentials')
      suite.assign(repo)
      const request = {
        remote: 'origin',
        expected_url: `${suite.service.url}/fork/repo.git`,
        recurse_submodules: false,
      }
      const preview = suite.rpc(repo, 'renderer_fetch_preview', { request })
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'renderer_fetch', { request, basis: preview.basis })
          .code,
        0
      )
      suite.assertActor()
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(repo)
    }, 120_000)

    it('scopes reviewed HTTPS pulls to the assigned account and fences a changed rewrite', () => {
      // Keep this Git integration independent of the user's Git environment,
      // template hooks, and proxy configuration. The service only binds loopback.
      const savedEnv = { ...suite.env }
      const templates = path.join(suite.root, 'pull-empty-templates')
      mkdirSync(templates)
      for (const key of Object.keys(suite.env)) {
        if (
          /^GIT_/i.test(key) ||
          /^(https?_proxy|all_proxy|no_proxy)$/i.test(key)
        ) {
          delete suite.env[key]
        }
      }
      Object.assign(suite.env, {
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: suite.globalConfig,
        GIT_TERMINAL_PROMPT: '0',
        GIT_TEMPLATE_DIR: templates,
        GIT_SSL_CAINFO: suite.service.cert,
        GIT_ASKPASS: 'false',
      })
      try {
        suite.configure({ host: suite.service.host })
        const repo = suite.seed('reviewed-pull-credentials')
        repo.git('config', 'branch.main.remote', 'origin')
        repo.git('config', 'branch.main.merge', 'refs/heads/main')
        suite.assign(repo)
        const remote = path.join(suite.service.projects, 'fork/repo.git')
        const writer = path.join(suite.root, 'reviewed-pull-writer')
        const git = (...args: string[]): string => {
          const result = spawnSync(suite.realGit, args, {
            env: suite.env,
            encoding: 'utf8',
            timeout: 20_000,
          })
          assert.ifError(result.error)
          assert.equal(result.status, 0, result.stderr)
          return result.stdout.trim()
        }
        git('clone', '-q', '--', remote, writer)
        git('-C', writer, 'config', 'user.name', 'HTTPS Writer')
        git('-C', writer, 'config', 'user.email', 'writer@example.invalid')
        git('-C', writer, 'config', 'commit.gpgsign', 'false')
        const advance = (content: string): void => {
          writeFileSync(path.join(writer, 'file'), content)
          git('-C', writer, 'add', 'file')
          git('-C', writer, 'commit', '-qm', 'update fixture')
          git('-C', writer, 'push', '-q', 'origin', 'main')
        }
        const request = {
          remote: 'origin',
          expected_url: `${suite.service.url}/fork/repo.git`,
          no_verify: false,
          report_progress: false,
          require_hook_callbacks: false,
        }
        advance('assigned Alice\n')
        const hook = path.join(repo.root, '.git/hooks/post-merge')
        const encoded = Buffer.from(`AliceCase:${ALICE}`).toString('base64')
        const probeResult = path.join(
          suite.root,
          'pull-hook-credential-result.json'
        )
        Object.assign(suite.env, {
          CREDENTIALS_PULL_ACCOUNT: 'AliceCase',
          CREDENTIALS_PULL_HOST: suite.service.host,
          CREDENTIALS_PULL_PROBE: path.resolve(
            'rust/tests/fixtures/pull-credential-hook.ts'
          ),
          CREDENTIALS_PULL_PROBE_OUTPUT: probeResult,
        })
        mkdirSync(path.dirname(hook), { recursive: true })
        writeFileSync(
          hook,
          `#!/bin/sh\nprintf '%s\\n' '${ALICE}' 'Basic ${encoded}' >&2\nexec "$ACCOUNT_NODE" "$CREDENTIALS_PULL_PROBE"\n`,
          { mode: 0o700 }
        )
        const preview = suite.rpc(repo, 'renderer_pull_preview', { request })
        suite.clearTransport()
        const invocation = suite.execute(['-C', repo.root, 'repo', '-'], {
          payload: JSON.stringify({
            method: 'renderer_pull',
            request,
            basis: preview.basis,
          }),
        })
        assert.ok(!invocation.stdout.includes(encoded))
        assert.ok(!invocation.stderr.includes(encoded))
        const result = object(JSON.parse(invocation.stdout))
        assert.equal(result.code, 0)
        assert.equal(result.error_kind, null)
        assert.equal(
          readFileSync(path.join(repo.root, 'file'), 'utf8'),
          'assigned Alice\n'
        )
        suite.assertActor()
        suite.assertPrivateStorage(repo)
        assert.ok(
          !JSON.stringify({
            service: suite.service.records(),
            helper: suite.records(suite.nativeLog),
            git: suite.records(suite.gitLog),
          }).includes(encoded)
        )
        assert.deepEqual(JSON.parse(readFileSync(probeResult, 'utf8')), {
          ownToken: true,
          deniedOther: true,
          deniedPrefix: true,
        })

        // A rewrite changed after review cannot send Alice's grant to another
        // repository or update the worktree using the stale decision.
        const stale = suite.rpc(repo, 'renderer_pull_preview', { request })
        const rewrite = `url.${suite.service.url}/other/.insteadOf`
        repo.git('config', rewrite, `${suite.service.url}/fork/`)
        const before = repo.git('rev-parse', 'HEAD').toString()
        suite.clearTransport()
        try {
          suite.rpc(
            repo,
            'renderer_pull',
            { request, basis: stale.basis },
            { fail: true }
          )
          assert.equal(repo.git('rev-parse', 'HEAD').toString(), before)
          assert.ok(!suite.service.records().some(row => row.actor !== null))
          assert.ok(!existsSync(suite.nativeLog))
        } finally {
          repo.git('config', '--unset-all', rewrite)
        }

        // Switching the assignment changes the token used by the same route.
        suite.assign(repo, OTHER)
        suite.service.update({
          allowed: {
            ...suite.service.control.allowed,
            '/fork/repo.git': 'bobcase',
          },
        })
        suite.env.CREDENTIALS_PULL_ACCOUNT = 'BobCase'
        advance('assigned Bob\n')
        const bobPreview = suite.rpc(repo, 'renderer_pull_preview', { request })
        rmSync(probeResult)
        suite.clearTransport()
        assert.equal(
          suite.rpc(repo, 'renderer_pull', { request, basis: bobPreview.basis })
            .code,
          0
        )
        assert.equal(
          readFileSync(path.join(repo.root, 'file'), 'utf8'),
          'assigned Bob\n'
        )
        suite.assertActor('bobcase')
        suite.assertPrivateStorage(repo)
        assert.deepEqual(JSON.parse(readFileSync(probeResult, 'utf8')), {
          ownToken: true,
          deniedOther: true,
          deniedPrefix: true,
        })

        suite.assign(repo, NONE)
        const disabledPreview = suite.rpc(repo, 'renderer_pull_preview', {
          request,
        })
        const disabledHead = repo.git('rev-parse', 'HEAD').toString()
        suite.clearTransport()
        suite.rpc(
          repo,
          'renderer_pull',
          { request, basis: disabledPreview.basis },
          { fail: true }
        )
        assert.equal(repo.git('rev-parse', 'HEAD').toString(), disabledHead)
        assert.ok(!suite.service.records().some(row => row.actor !== null))
        assert.ok(!existsSync(suite.nativeLog))

        // A previously assigned but signed-out account must not fall back to
        // the ambient user or a native helper.
        suite.assign(repo)
        suite.update({ signed_out: ['AliceCase'] })
        const signedOutPreview = suite.rpc(repo, 'renderer_pull_preview', {
          request,
        })
        const signedOutHead = repo.git('rev-parse', 'HEAD').toString()
        suite.clearTransport()
        suite.rpc(
          repo,
          'renderer_pull',
          { request, basis: signedOutPreview.basis },
          { fail: true }
        )
        assert.equal(repo.git('rev-parse', 'HEAD').toString(), signedOutHead)
        assert.ok(!suite.service.records().some(row => row.actor !== null))
        assert.ok(!existsSync(suite.nativeLog))
        suite.update({ signed_out: [] })

        // Current recursive Pull deliberately refuses existing gitlinks
        // before any transfer; parent credentials must not reach a child.
        const oid = repo.git('rev-parse', 'HEAD').toString().trim()
        repo.git('update-index', '--add', '--cacheinfo', `160000,${oid},nested`)
        const indexBefore = repo.git('ls-files', '--stage', '-z')
        suite.clearTransport()
        const blocked = suite.rpc(repo, 'renderer_pull_preview', { request })
        assert.equal(blocked.kind, 'unsupported_submodules')
        suite.rpc(
          repo,
          'renderer_pull',
          { request, basis: 'unused' },
          { fail: true }
        )
        assert.deepEqual(repo.git('ls-files', '--stage', '-z'), indexBefore)
        assert.ok(!suite.service.records().some(row => row.actor !== null))
        assert.ok(!existsSync(suite.nativeLog))
      } finally {
        for (const key of Object.keys(suite.env)) delete suite.env[key]
        Object.assign(suite.env, savedEnv)
      }
    }, 120_000)

    it('scopes assigned credentials to reviewed remote HEAD discovery', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('reviewed-remote-head-credentials')
      suite.assign(repo)
      const request = {
        remote: 'origin',
        expected_url: `${suite.service.url}/fork/repo.git`,
        recurse_submodules: false,
      }
      const fetch = suite.rpc(repo, 'renderer_fetch_preview', { request })
      assert.equal(
        suite.rpc(repo, 'renderer_fetch', { request, basis: fetch.basis }).code,
        0
      )
      const preview = suite.rpc(repo, 'renderer_remote_head_preview', {
        request,
      })
      suite.clearTransport()
      const result = suite.rpc(repo, 'renderer_remote_head', {
        request,
        basis: preview.basis,
      })
      assert.ok(result.code === 0 || result.code === 1)
      suite.assertActor()
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(repo)
    }, 120_000)

    it('limits rewritten destinations and rejects multiple push destinations', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('credential-destinations')
      suite.assign(repo)
      const original = `${suite.service.url}/fork/repo.git`
      repo.git(
        'remote',
        'set-url',
        'origin',
        'saved-account-alias:fork/repo.git'
      )
      repo.git(
        'config',
        `url.${suite.service.url}/.insteadOf`,
        'saved-account-alias:'
      )
      suite.clearTransport()
      assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
      suite.assertActor()
      repo.git('remote', 'set-url', 'origin', original)
      repo.git('remote', 'set-url', '--add', '--push', 'origin', original)
      repo.git(
        'remote',
        'set-url',
        '--add',
        '--push',
        'origin',
        `${suite.service.url}/other/repo.git`
      )
      suite.clearTransport()
      suite.rpc(
        repo,
        'push',
        { remote: 'origin', branch: 'main' },
        { fail: true }
      )
      assert.ok(
        !suite.service
          .records()
          .some(
            row =>
              String(row.path).startsWith('/other/repo.git') &&
              row.actor === 'alicecase'
          )
      )
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(repo)
    }, 90_000)
    it('rejects conflicting authorization headers while preserving unrelated headers', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('credential-header-conflicts')
      suite.assign(repo)
      const header = `Authorization: Bearer ${BOB}`
      const rejected = (): void => {
        const stored = readFileSync(suite.registry)
        const before = suite.evidence(repo)
        suite.clearTransport()
        suite.rpc(repo, 'fetch', { remote: 'origin' }, { fail: true })
        assert.ok(!suite.service.records().some(row => row.actor !== null))
        assert.ok(!existsSync(suite.nativeLog))
        assert.deepEqual(readFileSync(suite.registry), stored)
        assert.deepEqual(suite.evidence(repo), before)
      }
      for (const key of [
        'http.extraHeader',
        `http.${suite.service.url}.extraHeader`,
        `http.${suite.service.url}/fork/repo.git.extraHeader`,
      ]) {
        repo.git('config', key, header)
        try {
          rejected()
        } finally {
          repo.git('config', '--unset-all', key)
        }
      }
      Object.assign(suite.env, {
        GIT_CONFIG_COUNT: '1',
        GIT_CONFIG_KEY_0: `http.${suite.service.url}.extraHeader`,
        GIT_CONFIG_VALUE_0: header,
      })
      try {
        rejected()
      } finally {
        for (const key of [
          'GIT_CONFIG_COUNT',
          'GIT_CONFIG_KEY_0',
          'GIT_CONFIG_VALUE_0',
        ])
          delete suite.env[key]
      }
      const key = 'http.https://unrelated.invalid.extraHeader'
      repo.git('config', key, header)
      try {
        const stored = readFileSync(suite.registry)
        const configured = readFileSync(path.join(repo.root, '.git', 'config'))
        suite.clearTransport()
        assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
        suite.assertActor()
        assert.equal(repo.git('config', '--get', key).toString().trim(), header)
        assert.deepEqual(readFileSync(suite.registry), stored)
        assert.deepEqual(
          readFileSync(path.join(repo.root, '.git', 'config')),
          configured
        )
      } finally {
        repo.git('config', '--unset-all', key)
      }
      suite.assertPrivateStorage(repo)
    }, 90_000)
    it('uses native effective and SSH credentials without leaking assigned credentials', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('native-credential-preservation')
      suite.assign(repo, EFFECTIVE)
      suite.service.update({
        allowed: {
          ...suite.service.control.allowed,
          '/fork/repo.git': 'bobcase',
        },
      })
      suite.clearTransport()
      assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
      const rows = suite.service.records().filter(row => row.actor !== null)
      assert.ok(rows.length && rows.every(row => row.actor === 'bobcase'))
      assert.ok(
        suite.records(suite.nativeLog).some(row => row.action === 'get')
      )
      suite.assign(repo)
      const log = path.join(suite.root, 'native-ssh.jsonl')
      const ssh = path.join(suite.root, 'bin', 'ssh')
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
      repo.git(
        'remote',
        'add',
        'native-ssh',
        `ssh://git@${suite.service.host}/fork/repo.git`
      )
      suite.clearTransport()
      try {
        assert.equal(suite.rpc(repo, 'fetch', { remote: 'native-ssh' }).code, 0)
      } finally {
        delete suite.env.GIT_SSH
        delete suite.env.GIT_SSH_VARIANT
      }
      const calls = suite.records(log)
      assert.ok(
        calls.length &&
          calls.every(row => row.argv_clean && row.alice_environment_clean)
      )
      assert.ok(!suite.service.records().some(row => row.actor !== null))
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(repo)
    }, 90_000)
    it('revokes pending credential leases when account, remote, registration, or cancellation changes', async () => {
      for (const change of ['account', 'source', 'forget', 'cancel']) {
        suite.configure({ host: suite.service.host })
        const repo = suite.seed(`credential-revoke-${change}`)
        suite.assign(repo)
        suite.clearTransport()
        rmSync(suite.gateStarted, { force: true })
        rmSync(suite.gateRelease, { force: true })
        privateFile(suite.control, JSON.stringify({ gate: 'fetch' }))
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
        child.stdin.end(JSON.stringify({ method: 'fetch', remote: 'origin' }))
        try {
          const deadline = Date.now() + 25_000
          while (!existsSync(suite.gateStarted) && Date.now() < deadline)
            await new Promise(resolve => setTimeout(resolve, 20))
          assert.ok(
            existsSync(suite.gateStarted),
            'Git did not reach the operation gate'
          )
          if (change === 'account') suite.assign(repo, NONE)
          else if (change === 'source')
            repo.git(
              'remote',
              'set-url',
              'origin',
              `${suite.service.url}/other/repo.git`
            )
          else if (change === 'forget')
            suite.cli(['repositories', 'forget', repo.root])
          else child.kill('SIGINT')
          const stored = readFileSync(suite.registry)
          const before = suite.evidence(repo)
          if (change !== 'cancel') writeFileSync(suite.gateRelease, 'release')
          const code = await Promise.race([
            exited,
            new Promise<never>((_, reject) => {
              const timer = setTimeout(
                () => reject(new Error('Credential operation did not stop')),
                15_000
              )
              timer.unref()
            }),
          ])
          clean(stdout)
          clean(stderr)
          assert.notEqual(code, 0, change)
          assert.ok(!suite.service.records().some(row => row.actor !== null))
          assert.ok(!existsSync(suite.nativeLog))
          assert.deepEqual(readFileSync(suite.registry), stored)
          assert.deepEqual(suite.evidence(repo), before)
        } finally {
          writeFileSync(suite.gateRelease, 'release')
          privateFile(suite.control, '{}')
          if (child.exitCode === null && child.signalCode === null)
            child.kill('SIGKILL')
        }
        suite.assertPrivateStorage(repo)
      }
    }, 120_000)

    it('restricts helper protocol inputs and prevents submodules borrowing the parent lease', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('credential-protocol')
      suite.assign(repo)
      const childPath = path.join(repo.root, 'child')
      repo.git(
        '-c',
        'protocol.file.allow=always',
        'submodule',
        'add',
        '-q',
        path.join(suite.service.projects, 'fork/repo.git'),
        'child'
      )
      const child = repo.at(childPath)
      child.git(
        'remote',
        'set-url',
        'origin',
        `${suite.service.url}/fork/repo.git`
      )
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'submodule')
      const control = path.join(suite.root, 'credential-probes.json')
      const output = path.join(suite.root, 'credential-probes-result.json')
      const gitDirAlias = path.join(suite.root, 'parent-git-dir-alias')
      if (process.platform !== 'win32')
        symlinkSync(path.join(repo.root, '.git'), gitDirAlias)
      const base = `protocol=https\nhost=${suite.service.host}\n`
      const valid = [
        `${base}path=fork/repo.git\n\n`,
        `${base}path=fork/repo\n\n`,
        `${base}path=fork/repo.git/info/lfs\n\n`,
        `${base}path=fork/repo.git\n\n`.replaceAll('\n', '\r\n'),
      ]
      const invalid = [
        ['host-only', `${base}\n`],
        ['other-repository', `${base}path=other/repo.git\n\n`],
        ['repository-prefix', `${base}path=fork/repo-other.git\n\n`],
        ['lfs-prefix', `${base}path=fork/repo.git/info/lfs-evil\n\n`],
        [
          'lfs-extra',
          `${base}path=fork/repo.git/info/lfs/objects/arbitrary\n\n`,
        ],
        ['encoded-separator', `${base}path=fork/repo%2fother.git\n\n`],
        ['encoded-backslash', `${base}path=fork/repo%5cother.git\n\n`],
        ['encoded-dotdot', `${base}path=fork/%2e%2e\n\n`],
      ]
      const rawInvalid = [
        [
          'wrong-port',
          'protocol=https\nhost=127.0.0.1:1\npath=fork/repo.git\n\n',
        ],
        [
          'http',
          `protocol=http\nhost=${suite.service.host}\npath=fork/repo.git\n\n`,
        ],
        ['duplicate-protocol', `protocol=https\n${valid[0]}`],
        ['duplicate-host', `host=${suite.service.host}\n${valid[0]}`],
        ['duplicate-path', `path=fork/repo.git\n${valid[0]}`],
        ['carriage-return', `${base}path=fork/re\rpo.git\n\n`],
        ['nul', `${base}path=fork/repo.git\0\n\n`],
        ['oversize', `${base}path=${'x'.repeat(70_000)}\n\n`],
      ]
      privateFile(
        control,
        JSON.stringify({
          valid,
          invalid,
          rawInvalid,
          child: childPath,
          childGitDir: child
            .git('rev-parse', '--absolute-git-dir')
            .toString()
            .trim(),
          parentGitDir: repo
            .git('rev-parse', '--absolute-git-dir')
            .toString()
            .trim(),
          parentGitDirAlias: process.platform === 'win32' ? null : gitDirAlias,
          output,
          scenario: suite.scenario,
          host: suite.service.host,
        })
      )
      const hook = path.join(repo.root, '.git', 'hooks', 'pre-push')
      writeFileSync(
        hook,
        '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_PROBE_FIXTURE"\n',
        { mode: 0o700 }
      )
      Object.assign(suite.env, {
        CREDENTIALS_PROBE_CONTROL: control,
        CREDENTIALS_PROBE_FIXTURE: path.resolve(
          'rust/tests/fixtures/credential-probe.ts'
        ),
      })
      const stored = readFileSync(suite.registry)
      const globalBefore = readFileSync(suite.globalConfig)
      suite.clearTransport()
      const result = suite.rpc(repo, 'push', {
        remote: 'origin',
        branch: 'main',
      })
      const rows: Array<{ ok: boolean }> = existsSync(output)
        ? JSON.parse(readFileSync(output, 'utf8'))
        : []
      assert.equal(result.code, 0)
      assert.ok(rows.length && rows.every(row => row.ok), JSON.stringify(rows))
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.deepEqual(readFileSync(suite.globalConfig), globalBefore)
      suite.assertActor()
      suite.assertPrivateStorage(repo)
      suite.cli(['repositories', 'add', childPath])
      suite.assign(child, OTHER)
      suite.service.update({
        allowed: {
          ...suite.service.control.allowed,
          '/fork/repo.git': 'bobcase',
        },
      })
      suite.clearTransport()
      assert.equal(suite.rpc(child, 'fetch', { remote: 'origin' }).code, 0)
      suite.assertActor('bobcase')
    }, 120_000)

    it('uses the assigned account from a linked worktree', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('credential-linked-worktree')
      const linkedPath = path.join(
        suite.root,
        'credential-linked-worktree-checkout'
      )
      repo.git('worktree', 'add', '-qb', 'linked-test', linkedPath, 'HEAD')
      const linked = repo.at(linkedPath)
      suite.cli(['repositories', 'add', linkedPath])
      suite.assign(linked)
      suite.clearTransport()
      assert.equal(suite.rpc(linked, 'fetch', { remote: 'origin' }).code, 0)
      suite.assertActor()
      suite.assertPrivateStorage(linked)
    }, 120_000)

    it('refuses parent credentials during a real recursive submodule fetch', () => {
      suite.configure({ host: suite.service.host })
      const source = suite.seed('recursive-credential-source')
      const bare = path.join(suite.service.projects, 'fork/repo.git')
      source.git(
        '-c',
        'protocol.file.allow=always',
        'submodule',
        'add',
        '-q',
        bare,
        'child'
      )
      source.git(
        'config',
        '-f',
        '.gitmodules',
        'submodule.child.url',
        `${suite.service.url}/fork/repo.git`
      )
      source.git('add', '--all')
      source.git('commit', '-qm', 'add submodule')
      source.git('push', '-q', bare, 'HEAD:refs/heads/main')

      const targetPath = path.join(suite.root, 'recursive-credential-target')
      const clone = spawnSync(
        suite.realGit,
        ['clone', '-q', bare, targetPath],
        { env: suite.env, encoding: 'utf8', timeout: 20_000 }
      )
      assert.ifError(clone.error)
      assert.equal(clone.status, 0, clone.stderr)
      const target = source.at(targetPath)
      const baselinePath = path.join(
        suite.root,
        'recursive-credential-baseline'
      )
      const baselineClone = spawnSync(
        suite.realGit,
        ['clone', '-q', bare, baselinePath],
        { env: suite.env, encoding: 'utf8', timeout: 20_000 }
      )
      assert.ifError(baselineClone.error)
      assert.equal(baselineClone.status, 0, baselineClone.stderr)
      const baseline = source.at(baselinePath)
      baseline.git(
        'remote',
        'set-url',
        'origin',
        `${suite.service.url}/fork/repo.git`
      )
      suite.cli(['repositories', 'add', baselinePath])
      suite.assign(baseline)
      target.git('config', 'submodule.child.url', bare)
      target.git(
        '-c',
        'protocol.file.allow=always',
        'submodule',
        'update',
        '--init',
        '--',
        'child'
      )
      target
        .at(path.join(targetPath, 'child'))
        .git(
          'remote',
          'set-url',
          'origin',
          `${suite.service.url}/fork/repo.git`
        )
      target.git(
        'remote',
        'set-url',
        'origin',
        `${suite.service.url}/fork/repo.git`
      )
      suite.cli(['repositories', 'add', targetPath])
      suite.assign(target)

      const child = source.at(path.join(source.root, 'child'))
      child.write('advance.txt', 'new submodule commit\n')
      child.git('add', 'advance.txt')
      child.git(
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.invalid',
        '-c',
        'commit.gpgsign=false',
        'commit',
        '-qm',
        'advance'
      )
      child.git('push', '-q', bare, 'HEAD:refs/heads/subfixture')
      source.git('add', 'child')
      source.git('commit', '-qm', 'advance submodule')
      source.git('push', '-q', bare, 'HEAD:refs/heads/main')

      const request = {
        remote: 'origin',
        expected_url: `${suite.service.url}/fork/repo.git`,
        recurse_submodules: true,
      }
      const baselineRequest = { ...request, recurse_submodules: false }
      const baselinePreview = suite.rpc(baseline, 'renderer_fetch_preview', {
        request: baselineRequest,
      })
      suite.clearTransport()
      assert.equal(
        suite.rpc(baseline, 'renderer_fetch', {
          request: baselineRequest,
          basis: baselinePreview.basis,
        }).code,
        0
      )
      const baselineRecords = suite.service.records()
      const preview = suite.rpc(target, 'renderer_fetch_preview', { request })
      suite.clearTransport()
      const result = suite.rpc(
        target,
        'renderer_fetch',
        { request, basis: preview.basis },
        { fail: true }
      )
      assert.match(
        String(result.stderr || result.stdout),
        /"code":1/,
        'nested fetch should be refused without its own account'
      )
      const records = suite.service.records()
      assert.ok(
        records.some(row => row.actor === 'alicecase'),
        'parent fetch must authenticate'
      )
      assert.deepEqual(
        records.filter(row => row.actor !== null),
        baselineRecords.filter(row => row.actor !== null),
        'submodule must not authenticate with the parent account'
      )
      const wanted = child.git('rev-parse', 'HEAD').toString().trim()
      const nestedObject = spawnSync(
        suite.realGit,
        [
          '-C',
          path.join(targetPath, 'child'),
          'cat-file',
          '-e',
          `${wanted}^{commit}`,
        ],
        { env: { ...suite.env, GIT_NO_LAZY_FETCH: '1' }, timeout: 10_000 }
      )
      assert.notEqual(
        nestedObject.status,
        0,
        'the protected submodule commit must remain unfetched'
      )
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(target)
    }, 120_000)

    it('does not lend the parent account to renderer submodule updates', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('renderer-submodule-credentials')
      suite.assign(repo)
      repo.git(
        '-c',
        'protocol.file.allow=always',
        'submodule',
        'add',
        '-q',
        path.join(suite.service.projects, 'fork/repo.git'),
        'child'
      )
      repo.git('submodule', 'deinit', '-f', '--', 'child')
      rmSync(path.join(repo.root, '.git', 'modules', 'child'), {
        recursive: true,
        force: true,
      })
      repo.git(
        'config',
        '-f',
        '.gitmodules',
        'submodule.child.url',
        `${suite.service.url}/fork/repo.git`
      )
      suite.clearTransport()
      const result = suite.rpc(
        repo,
        'renderer_update_submodules',
        { allow_file_protocol: false },
        { fail: true }
      )
      assert.match(
        String(result.stderr || result.stdout),
        /credentials refused/i
      )
      assert.ok(!suite.service.records().some(row => row.actor !== null))
      assert.ok(!existsSync(suite.nativeLog))
      suite.assertPrivateStorage(repo)
    }, 120_000)

    it('scopes same-repository credentials for tracking-branch fast-forward hooks', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('tracking-fast-forward-credentials')
      const old = repo.git('rev-parse', 'HEAD').toString().trim()
      repo.git('branch', 'broker-reference-hook', old)
      repo.git(
        'branch',
        '--set-upstream-to',
        'origin/fork-main',
        'broker-reference-hook'
      )
      repo.write('file', 'new upstream content\n')
      repo.git('commit', '-qam', 'upstream')
      const next = repo.git('rev-parse', 'HEAD').toString().trim()
      repo.git('update-ref', 'refs/remotes/origin/fork-main', next)
      suite.assign(repo)
      const sentinel = 'synthetic-private-caller-configuration'
      suite.update({ fixture_secret: sentinel })
      const inherited = suite.env.GIT_CONFIG_PARAMETERS
      suite.env.GIT_CONFIG_PARAMETERS = `'desktop.fixtureSecret=${sentinel}'`
      const output = path.join(suite.root, 'fast-forward-hook-result.json')
      writeFileSync(
        path.join(repo.root, '.git', 'hooks', 'reference-transaction'),
        '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_REFERENCE_FIXTURE" "$@"\n',
        { mode: 0o700 }
      )
      Object.assign(suite.env, {
        CREDENTIALS_REFERENCE_FIXTURE: path.resolve(
          'rust/tests/fixtures/reference-credential-hook.ts'
        ),
        CREDENTIALS_HOOK_OUTPUT: output,
        CREDENTIALS_HOOK_HOST: suite.service.host,
      })
      try {
        const request = {
          branches: [
            {
              ref: 'refs/heads/broker-reference-hook',
              sha: old,
              upstream_ref: 'refs/remotes/origin/fork-main',
              upstream_sha: next,
            },
          ],
          recurse_submodules: false,
        }
        const preview = suite.rpc(repo, 'renderer_fast_forward_preview', {
          request,
        })
        assert.equal(
          suite.rpc(repo, 'renderer_fast_forward', {
            request,
            basis: preview.basis,
          }).code,
          0
        )
        assert.deepEqual(JSON.parse(readFileSync(output, 'utf8')), {
          ok: true,
          caller_parameter: true,
          code: 0,
          phase: 'prepared',
        })
        assert.equal(
          repo.git('rev-parse', 'broker-reference-hook').toString().trim(),
          next
        )
        assert.ok(!existsSync(suite.nativeLog))
        suite.assertPrivateStorage(repo)
      } finally {
        if (inherited === undefined) delete suite.env.GIT_CONFIG_PARAMETERS
        else suite.env.GIT_CONFIG_PARAMETERS = inherited
      }
    }, 120_000)

    it('passes the scoped credential and caller configuration into reference transaction hooks', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('reference-hook-credentials')
      suite.assign(repo)
      const sentinel = 'synthetic-private-caller-configuration'
      suite.update({ fixture_secret: sentinel })
      const inherited = suite.env.GIT_CONFIG_PARAMETERS
      suite.env.GIT_CONFIG_PARAMETERS = `'desktop.fixtureSecret=${sentinel}'`
      const output = path.join(suite.root, 'reference-hook-result.json')
      const hook = path.join(
        repo.root,
        '.git',
        'hooks',
        'reference-transaction'
      )
      writeFileSync(
        hook,
        '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_REFERENCE_FIXTURE" "$@"\n',
        { mode: 0o700 }
      )
      Object.assign(suite.env, {
        CREDENTIALS_REFERENCE_FIXTURE: path.resolve(
          'rust/tests/fixtures/reference-credential-hook.ts'
        ),
        CREDENTIALS_HOOK_OUTPUT: output,
        CREDENTIALS_HOOK_HOST: suite.service.host,
      })
      try {
        const params = {
          name: 'broker-reference-hook',
          start_point: 'refs/heads/main',
          no_track: true,
        }
        const plan = suite.rpc(repo, 'create_branch_preview', params)
        const stored = readFileSync(suite.registry)
        const before = object(suite.evidence(repo))
        suite.clear()
        suite.clearTransport()
        assert.equal(
          suite.rpc(repo, 'create_branch', { ...params, basis: plan.basis })
            .code,
          0
        )
        assert.deepEqual(JSON.parse(readFileSync(output, 'utf8')), {
          ok: true,
          caller_parameter: true,
          code: 0,
          phase: 'prepared',
        })
        assert.deepEqual(
          repo.git('rev-parse', 'refs/heads/broker-reference-hook'),
          repo.git('rev-parse', 'HEAD')
        )
        const after = object(suite.evidence(repo))
        delete before.refs
        delete after.refs
        assert.deepEqual(readFileSync(suite.registry), stored)
        assert.deepEqual(after, before)
        assert.ok(!existsSync(suite.nativeLog))
        assert.ok(
          suite
            .calls()
            .some(row => row.kind === 'api' && row.actor === 'alicecase')
        )
        assert.ok(
          suite
            .records(suite.gitLog)
            .every(row => row.argv_clean && row.alice_environment_clean)
        )
        for (const entry of readdirSync(path.join(repo.root, '.git'), {
          recursive: true,
          withFileTypes: true,
        })) {
          if (entry.isDirectory() && entry.name.endsWith('.create-hooks')) {
            for (const script of readdirSync(
              path.join(entry.parentPath, entry.name),
              { withFileTypes: true }
            )) {
              if (script.isFile()) {
                const body = readFileSync(
                  path.join(script.parentPath, script.name)
                )
                assert.ok(!body.includes(sentinel) && !body.includes(ALICE))
              }
            }
          }
        }
        suite.assertPrivateStorage(repo)
      } finally {
        if (inherited === undefined) delete suite.env.GIT_CONFIG_PARAMETERS
        else suite.env.GIT_CONFIG_PARAMETERS = inherited
      }
    }, 90_000)

    it('validates staged and committed LFS configuration without changing the index', () => {
      const bad = `[lfs]\n url = https://embedded:${ALICE}@${suite.service.host}/fork/repo.git/info/lfs\n`
      const safe = `[lfs]\n url = ${suite.service.url}/fork/repo.git/info/lfs\n`
      for (const working of [false, true])
        for (const source of ['index', 'head']) {
          suite.configure({ host: suite.service.host })
          const repo = suite.seed(`credential-lfsconfig-${working}-${source}`)
          suite.assign(repo)
          const configuration = path.join(repo.root, '.lfsconfig')
          writeFileSync(configuration, bad)
          repo.git('add', '--', '.lfsconfig')
          if (source === 'head') {
            repo.git('commit', '-qm', 'malicious fixture LFS configuration')
            repo.git('rm', '--cached', '-q', '--', '.lfsconfig')
          }
          if (working) writeFileSync(configuration, safe)
          else rmSync(configuration)
          const stored = readFileSync(suite.registry)
          const before = suite.evidence(repo)
          const index = readFileSync(path.join(repo.root, '.git', 'index'))
          suite.clearTransport()
          if (working) {
            assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
            suite.assertActor()
            assert.equal(readFileSync(configuration, 'utf8'), safe)
            assert.deepEqual(
              readFileSync(path.join(repo.root, '.git', 'index')),
              index
            )
            suite.assertPrivateStorage(repo)
          } else {
            suite.rpc(repo, 'fetch', { remote: 'origin' }, { fail: true })
            assert.ok(!suite.service.records().some(row => row.actor !== null))
            assert.ok(!existsSync(suite.nativeLog))
            assert.deepEqual(suite.evidence(repo), before)
            assert.ok(!existsSync(configuration))
          }
          assert.deepEqual(readFileSync(suite.registry), stored)
        }
    }, 120_000)
  }
)
