// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  existsSync,
  readFileSync,
  readdirSync,
  rmSync,
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
