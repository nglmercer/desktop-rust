// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  existsSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { afterAll, beforeAll, describe, it } from 'vitest'
import {
  ALICE,
  BINARY,
  BOB,
  ENV,
  NONE,
  OTHER,
  clean,
  object,
} from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1')(
  'saved-account Git credentials',
  () => {
    let suite: Credentials
    beforeAll(async () => {
      suite = new Credentials()
      await suite.start()
    }, 150_000)
    afterAll(async () => {
      if (suite) await suite.close()
    })

    it('uses the assigned account for reviewed remote HEAD refresh', () => {
      const repo = suite.seed('assigned-remote-head')
      suite.assign(repo)
      assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
      const request = {
        remote: 'origin',
        expected_url: `https://${suite.service.host}/fork/repo.git`,
        recurse_submodules: false,
        is_background_task: true,
      }
      const preview = suite.rpc(repo, 'renderer_remote_head_preview', {
        request,
      })
      assert.equal(typeof preview.basis, 'string')
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'renderer_remote_head', {
          request,
          basis: preview.basis,
        }).code,
        0
      )
      suite.assertActor()
      assert.equal(
        repo.git('symbolic-ref', 'refs/remotes/origin/HEAD').toString().trim(),
        'refs/remotes/origin/main'
      )
      suite.assertPrivateStorage(repo)
    })

    it('uses the assigned identity for fetch, push and pull and redacts remote errors', () => {
      const repo = suite.seed('named-git')
      suite.assign(repo)
      const globalBefore = readFileSync(suite.globalConfig)
      suite.clearTransport()
      assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
      suite.assertActor()
      suite.env.GIT_CONFIG_PARAMETERS = `'${`credential.helper=${suite.helperCommand}`.replaceAll("'", "'\\''")}'`
      try {
        suite.clearTransport()
        assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
        suite.assertActor()
      } finally {
        delete suite.env.GIT_CONFIG_PARAMETERS
      }
      repo.write('file', 'named account local push\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'local push')
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'push', { remote: 'origin', branch: 'main' }).code,
        0
      )
      suite.assertActor()
      const remote = path.join(suite.service.projects, 'fork/repo.git')
      const run = (...args: string[]): string => {
        const result = spawnSync(suite.realGit, args, {
          env: suite.env,
          encoding: 'utf8',
          timeout: 20_000,
        })
        assert.equal(result.status, 0, result.stderr)
        return result.stdout
      }
      assert.equal(
        run('-C', remote, 'rev-parse', 'refs/heads/main'),
        repo.git('rev-parse', 'HEAD').toString()
      )
      const writer = path.join(suite.root, 'remote-writer')
      run('clone', '-q', remote, writer)
      run('-C', writer, 'config', 'user.name', 'HTTPS Writer')
      run('-C', writer, 'config', 'user.email', 'writer@example.invalid')
      writeFileSync(path.join(writer, 'file'), 'remote fast forward\n')
      run('-C', writer, 'commit', '-qam', 'remote forward')
      run('-C', writer, 'push', '-q', 'origin', 'main')
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'pull', { remote: 'origin', branch: 'main' }).code,
        0
      )
      suite.assertActor()
      assert.equal(
        readFileSync(path.join(repo.root, 'file'), 'utf8'),
        'remote fast forward\n'
      )
      repo.write('file', 'local rejected diagnostic push\n')
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'rejected push')
      const encoded = Buffer.from(`AliceCase:${ALICE}`).toString('base64')
      const hook = path.join(remote, 'hooks', 'pre-receive')
      writeFileSync(
        hook,
        `#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' 'synthetic remote diagnostic ${ALICE}' 'Basic ${encoded}' >&2\nexit 1\n`,
        { mode: 0o700 }
      )
      const stored = readFileSync(suite.registry)
      const before = suite.evidence(repo)
      suite.clearTransport()
      try {
        const rejected = suite.rpc(
          repo,
          'push',
          { remote: 'origin', branch: 'main' },
          { fail: true }
        )
        assert.ok(!JSON.stringify(rejected).includes(encoded))
      } finally {
        rmSync(hook, { force: true })
      }
      suite.assertActor()
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.deepEqual(suite.evidence(repo), before)
      suite.assign(repo, OTHER)
      suite.service.update({
        allowed: {
          ...suite.service.control.allowed,
          '/fork/repo.git': 'bobcase',
        },
      })
      suite.clearTransport()
      assert.equal(suite.rpc(repo, 'fetch', { remote: 'origin' }).code, 0)
      suite.assertActor('bobcase')
      assert.deepEqual(readFileSync(suite.globalConfig), globalBefore)
      suite.assertPrivateStorage(repo)
    }, 120_000)

    it('blocks transfers for disabled and signed-out accounts without changing repository state', () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('disabled-git')
      suite.assign(repo, NONE)
      let stored = readFileSync(suite.registry)
      let before = suite.evidence(repo)
      for (const [method, params] of [
        ['fetch', { remote: 'origin' }],
        ['pull', { remote: 'origin', branch: 'main' }],
        ['push', { remote: 'origin', branch: 'main' }],
      ] as const) {
        suite.clearTransport()
        suite.rpc(repo, method, params, { fail: true })
        assert.ok(!suite.service.records().some(row => row.actor !== null))
        assert.ok(!existsSync(suite.nativeLog))
        assert.deepEqual(readFileSync(suite.registry), stored)
        assert.deepEqual(suite.evidence(repo), before)
      }
      suite.assign(repo)
      suite.update({ signed_out: ['AliceCase'] })
      stored = readFileSync(suite.registry)
      before = suite.evidence(repo)
      suite.clearTransport()
      suite.rpc(repo, 'fetch', { remote: 'origin' }, { fail: true })
      assert.ok(!suite.service.records().some(row => row.actor !== null))
      assert.ok(!existsSync(suite.nativeLog))
      assert.deepEqual(readFileSync(suite.registry), stored)
      assert.equal(object(suite.account(repo).assignment).id, 101)
      assert.deepEqual(suite.evidence(repo), before)
      suite.assertPrivateStorage(repo)
    }, 90_000)

    it('keeps credentials scoped through real LFS transfers, failures and cancellation', async () => {
      suite.configure({ host: suite.service.host })
      const repo = suite.seed('named-lfs')
      repo.git('lfs', 'install', '--local')
      repo.git('lfs', 'track', '*.asset')
      const digest = (value: Buffer): string =>
        createHash('sha256').update(value).digest('hex')
      const pointer = (value: Buffer): Buffer =>
        Buffer.from(
          `version https://git-lfs.github.com/spec/v1\noid sha256:${digest(value)}\nsize ${value.length}\n`
        )
      const media = repo
        .git('lfs', 'env')
        .toString()
        .split('\n')
        .find(line => line.startsWith('LocalMediaDir='))
        ?.slice('LocalMediaDir='.length)
      assert.ok(media)
      const cached = (value: Buffer): string => {
        const oid = digest(value)
        return path.join(media, oid.slice(0, 2), oid.slice(2, 4), oid)
      }
      const payload = Buffer.from('direct named LFS payload\0\n')
      const asset = path.join(repo.root, 'large.asset')
      writeFileSync(asset, payload)
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'LFS payload')
      suite.assign(repo)
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'lfs_transfer', {
          request: { action: 'push', remote: 'origin' },
        }).code,
        0
      )
      assert.deepEqual(
        readFileSync(path.join(suite.service.objects, digest(payload))),
        payload
      )
      suite.assertActor()
      rmSync(cached(payload))
      writeFileSync(asset, pointer(payload))
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'lfs_transfer', {
          request: { action: 'fetch', remote: 'origin' },
        }).code,
        0
      )
      assert.deepEqual(readFileSync(cached(payload)), payload)
      assert.deepEqual(readFileSync(asset), pointer(payload))
      suite.assertActor()
      rmSync(cached(payload))
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'lfs_transfer', {
          request: { action: 'pull', remote: 'origin' },
        }).code,
        0
      )
      assert.deepEqual(readFileSync(asset), payload)
      suite.assertActor()
      const pushed = Buffer.from('pre-push inherited named LFS payload\n')
      writeFileSync(asset, pushed)
      repo.git('add', '--all')
      repo.git('commit', '-qm', 'LFS pre-push')
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'push', { remote: 'origin', branch: 'main' }).code,
        0
      )
      assert.deepEqual(
        readFileSync(path.join(suite.service.objects, digest(pushed))),
        pushed
      )
      assert.ok(
        suite.service
          .records()
          .some(
            row => row.actor !== null && String(row.path).includes('/info/lfs/')
          )
      )
      suite.assertActor()
      const pulled = Buffer.from('pull smudge inherited named LFS payload\n')
      writeFileSync(path.join(suite.service.objects, digest(pulled)), pulled)
      const remote = path.join(suite.service.projects, 'fork/repo.git')
      const env = {
        ...process.env,
        GIT_INDEX_FILE: path.join(suite.root, 'remote-lfs.index'),
        GIT_AUTHOR_NAME: 'Fixture Writer',
        GIT_AUTHOR_EMAIL: 'writer@example.invalid',
        GIT_COMMITTER_NAME: 'Fixture Writer',
        GIT_COMMITTER_EMAIL: 'writer@example.invalid',
      }
      const remoteGit = (args: string[], input?: Buffer): string => {
        const result = spawnSync(
          suite.realGit,
          ['--git-dir', remote, ...args],
          { input, env, encoding: 'buffer', timeout: 20_000 }
        )
        assert.equal(result.status, 0, result.stderr.toString())
        return result.stdout.toString().trim()
      }
      remoteGit(['read-tree', 'refs/heads/main'])
      const blob = remoteGit(['hash-object', '-w', '--stdin'], pointer(pulled))
      remoteGit([
        'update-index',
        '--add',
        '--cacheinfo',
        `100644,${blob},large.asset`,
      ])
      const tree = remoteGit(['write-tree'])
      const parent = remoteGit(['rev-parse', 'refs/heads/main'])
      const tip = remoteGit([
        'commit-tree',
        tree,
        '-p',
        parent,
        '-m',
        'remote pointer commit',
      ])
      remoteGit(['update-ref', 'refs/heads/main', tip])
      assert.ok(!existsSync(cached(pulled)))
      suite.clearTransport()
      assert.equal(
        suite.rpc(repo, 'pull', { remote: 'origin', branch: 'main' }).code,
        0
      )
      assert.deepEqual(readFileSync(asset), pulled)
      assert.deepEqual(readFileSync(cached(pulled)), pulled)
      assert.ok(
        suite.service
          .records()
          .some(
            row => row.actor !== null && String(row.path).includes('/info/lfs/')
          )
      )
      suite.assertActor()
      const storage = path.join(repo.root, '.git', 'lfs')
      assert.ok(!existsSync(path.join(storage, 'logs')))
      rmSync(cached(pulled))
      suite.clearTransport()
      suite.service.update({ rejectLfsBatch: true })
      const encoded = Buffer.from(`AliceCase:${ALICE}`).toString('base64')
      try {
        const rejected = suite.rpc(
          repo,
          'lfs_transfer',
          { request: { action: 'fetch', remote: 'origin' } },
          { fail: true }
        )
        assert.ok(!JSON.stringify(rejected).includes(encoded))
        const value = object(JSON.parse(String(rejected.stdout)))
        assert.notEqual(value.code, 0)
        assert.equal(value.error_kind, 'failed')
        assert.equal(value.stdout, '')
        assert.ok(
          suite.service
            .records()
            .some(
              row =>
                row.actor === 'alicecase' &&
                row.path === '/fork/repo.git/info/lfs/objects/batch' &&
                row.lfs_batch_rejected
            )
        )
      } finally {
        suite.service.update({ rejectLfsBatch: false })
      }
      suite.assertActor()
      assert.ok(!existsSync(cached(pulled)))
      assert.deepEqual(readFileSync(asset), pulled)
      const checkStorage = (): void => {
        assert.ok(!existsSync(path.join(storage, 'logs')))
        assert.ok(
          !readdirSync(storage).some(name => name.startsWith('.desktop-lfs-'))
        )
        for (const entry of readdirSync(storage, {
          recursive: true,
          withFileTypes: true,
        })) {
          if (entry.isFile()) {
            const contents = readFileSync(
              path.join(entry.parentPath, entry.name)
            )
            for (const secret of [ALICE, BOB, ENV, encoded])
              assert.ok(!contents.includes(secret))
          }
        }
      }
      checkStorage()
      const unrelated = path.join(repo.root, 'lfs-cancel-unrelated')
      writeFileSync(unrelated, 'unrelated cancellation sentinel\n')
      const before = suite.evidence(repo)
      const stored = readFileSync(suite.registry)
      const globalBefore = readFileSync(suite.globalConfig)
      suite.clearTransport()
      suite.service.update({
        blockPath: '/fork/repo.git/info/lfs/objects/batch',
      })
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
        JSON.stringify({
          method: 'lfs_transfer',
          request: { action: 'fetch', remote: 'origin' },
        })
      )
      try {
        const deadline = Date.now() + 25_000
        while (!existsSync(suite.service.started) && Date.now() < deadline)
          await new Promise(resolve => setTimeout(resolve, 20))
        assert.ok(
          existsSync(suite.service.started),
          'Git LFS did not reach the authenticated batch'
        )
        const aliases = readdirSync(storage).filter(name =>
          name.startsWith('.desktop-lfs-')
        )
        assert.ok(aliases.length)
        for (const alias of aliases)
          assert.ok(statSync(path.join(storage, alias, 'logs')).isFile())
        assert.ok(
          suite.service
            .records()
            .some(
              row =>
                row.actor === 'alicecase' &&
                row.path === '/fork/repo.git/info/lfs/objects/batch'
            )
        )
        const processes = spawnSync('ps', ['-A', '-o', 'pid=,ppid=,comm='], {
          encoding: 'utf8',
        })
          .stdout.trim()
          .split('\n')
          .map(line => line.trim().split(/\s+/, 3))
        const descendants = new Map<number, string>()
        let parents = new Set([child.pid])
        while (parents.size) {
          const found = processes.filter(
            ([pid, ppid]) =>
              parents.has(Number(ppid)) && !descendants.has(Number(pid))
          )
          parents = new Set(found.map(([pid]) => Number(pid)))
          for (const [pid, , name] of found) descendants.set(Number(pid), name)
        }
        assert.ok(
          [...descendants.values()].some(name => name === 'git-lfs'),
          'No real Git LFS child was blocked'
        )
        child.kill('SIGINT')
        assert.equal(
          await Promise.race([
            exited,
            new Promise<never>((_, reject) =>
              setTimeout(
                () => reject(new Error('Cancellation timed out')),
                15_000
              )
            ),
          ]),
          130
        )
        clean(stdout)
        clean(stderr)
        assert.ok(!(stdout + stderr).includes(encoded))
        const until = Date.now() + 10_000
        const alive = (pid: number): boolean => {
          try {
            const stat = readFileSync(`/proc/${pid}/stat`, 'utf8')
            return !/\) Z /.test(stat)
          } catch {
            return false
          }
        }
        while ([...descendants.keys()].some(alive) && Date.now() < until)
          await new Promise(resolve => setTimeout(resolve, 20))
        assert.ok(
          ![...descendants.keys()].some(alive),
          'Cancelled LFS left a child alive'
        )
        assert.deepEqual(suite.evidence(repo), before)
        assert.deepEqual(readFileSync(suite.registry), stored)
        assert.deepEqual(readFileSync(suite.globalConfig), globalBefore)
        assert.equal(
          readFileSync(unrelated, 'utf8'),
          'unrelated cancellation sentinel\n'
        )
        assert.deepEqual(readFileSync(asset), pulled)
        assert.deepEqual(readFileSync(cached(payload)), payload)
        assert.deepEqual(readFileSync(cached(pushed)), pushed)
        assert.ok(!existsSync(cached(pulled)))
        checkStorage()
        suite.assertActor()
      } finally {
        suite.service.update({ blockPath: undefined })
        writeFileSync(suite.service.release, 'release')
        if (child.exitCode === null && child.signalCode === null)
          child.kill('SIGKILL')
      }
      suite.assertPrivateStorage(repo)
    }, 150_000)
  }
)
