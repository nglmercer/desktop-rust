// @vitest-environment node
import { execFileSync } from 'node:child_process'
import { chmodSync, mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('dugite', () => ({
  exec: async (args: string[], path: string) => {
    try {
      return { exitCode: 0, stdout: execFileSync('git', args, { cwd: path, encoding: 'utf8' }), stderr: '' }
    } catch (error) {
      const result = error as { status?: number; stdout?: string; stderr?: string }
      return { exitCode: result.status ?? 1, stdout: result.stdout ?? '', stderr: result.stderr ?? '' }
    }
  },
}))

import { getRepoHooks } from '../../src/lib/hooks/get-repo-hooks'

describe.skipIf(process.platform === 'win32')('Git hook discovery', () => {
  let root: string | undefined
  afterEach(() => { if (root !== undefined) rmSync(root, { recursive: true, force: true }); root = undefined })

  it('finds executable symlinked hooks but skips missing and directory targets', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hooks-discovery-'))
    execFileSync('git', ['init', '-q', root])
    const hooks = join(root, '.git', 'hooks')
    const target = join(root, 'hook-target')
    writeFileSync(target, '#!/bin/sh\nexit 0\n')
    chmodSync(target, 0o755)
    symlinkSync(target, join(hooks, 'pre-commit'))
    symlinkSync(join(root, 'missing'), join(hooks, 'commit-msg'))
    mkdirSync(join(root, 'hook-directory'))
    symlinkSync(join(root, 'hook-directory'), join(hooks, 'post-commit'))
    expect(await Array.fromAsync(getRepoHooks(root, ['pre-commit', 'commit-msg', 'post-commit']))).toEqual(['pre-commit'])
    expect(await Array.fromAsync(getRepoHooks(root, ['*']))).toEqual(['pre-commit'])
  })
})
