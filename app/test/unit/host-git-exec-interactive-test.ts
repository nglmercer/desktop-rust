import assert from 'node:assert'
import { createHash } from 'node:crypto'
import { afterEach, beforeEach, describe, it } from 'vitest'
import { exec, spawn, ExecError } from '../../src/lib/git/host-git-exec'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'
import { Repository } from '../../src/models/repository'
import { backend } from '../../src/backend/client'
import { updateRefWithVerification } from '../../src/lib/git/update-ref'

describe('interactive host-backed git execution', () => {
  beforeEach(() => installNodeHostBridge())
  afterEach(() => uninstallNodeHostBridge())

  it('exposes the live child so callers can write stdin after spawn', async () => {
    const result = await exec(['hash-object', '--stdin'], process.cwd(), {
      processCallback: child => {
        child.stdin?.end('hello\n')
      },
    })
    assert.equal(result.exitCode, 0)
    assert.equal(
      result.stdout.trim(),
      'ce013625030ba8dba906f756967f9e9ca394464a'
    )
  })

  it('drives an update-ref transaction through stdout and stdin', async () => {
    const path = await mkdtemp(join(tmpdir(), 'desktop-interactive-ref-'))
    try {
      await exec(['init', '-b', 'main'], path)
      await exec(['config', 'user.name', 'Desktop Test'], path)
      await exec(['config', 'user.email', 'desktop@example.com'], path)
      await exec(['commit', '--allow-empty', '-m', 'initial'], path)
      const head = String(
        (await exec(['rev-parse', 'HEAD'], path)).stdout
      ).trim()
      const repository = new Repository(path, -1, null, false)

      await updateRefWithVerification(
        repository,
        'refs/heads/guarded',
        '0000000000000000000000000000000000000000',
        head,
        'test transaction',
        async () => {
          const context = await backend().repositoryAssistedCommitHead(path)
          assert.equal(context.kind, 'ok')
          if (context.kind === 'ok') {
            assert.equal(context.sha, head)
            assert.equal(context.ref, 'refs/heads/main')
          }
        }
      )

      const created = String(
        (await exec(['rev-parse', 'refs/heads/guarded'], path)).stdout
      ).trim()
      assert.equal(created, head)
    } finally {
      await rm(path, { recursive: true, force: true })
    }
  })

  it('rejects rather than hanging when the interactive child fails to spawn', async () => {
    await assert.rejects(
      exec(['--version'], join(tmpdir(), 'desktop-missing-interactive-cwd'), {
        processCallback: () => {},
      }),
      error => error instanceof ExecError
    )
  })

  it('accepts a final stdin chunk larger than the interactive write budget', async () => {
    const input = Buffer.alloc(1024 * 1024 + 1, 120)
    const result = await exec(['hash-object', '--stdin'], process.cwd(), {
      stdin: input,
      processCallback: () => {},
    })

    assert.equal(result.exitCode, 0)
    assert.equal(
      result.stdout.trim(),
      createHash('sha1')
        .update(Buffer.concat([Buffer.from(`blob ${input.length}\0`), input]))
        .digest('hex')
    )
  })

  it('exposes streaming git processes through the same host child surface', async () => {
    const child = spawn(['--version'], process.cwd())
    const output: string[] = []
    child.stdout?.setEncoding('utf8')
    child.stdout?.on('data', chunk => output.push(chunk))
    const exit = new Promise<number | null>(resolve =>
      child.once('exit', code => resolve(code))
    )

    assert.equal(await exit, 0)
    assert.match(output.join(''), /^git version /)
  })

  it('rejects a signal-terminated interactive child instead of reporting success', async () => {
    await assert.rejects(
      exec(['cat-file', '--batch'], process.cwd(), {
        processCallback: child => {
          child.once('spawn', () => child.kill('SIGTERM'))
        },
      }),
      error =>
        error instanceof ExecError &&
        error.signal === 'SIGTERM' &&
        error.killed === true
    )
  })

  it('preserves maxBuffer failure with partial output', async () => {
    await assert.rejects(
      exec(['--version'], process.cwd(), {
        maxBuffer: 2,
        processCallback: () => {},
      }),
      error =>
        error instanceof ExecError &&
        error.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER' &&
        typeof error.stdout === 'string' &&
        error.stdout.length > 0
    )
  })
})
