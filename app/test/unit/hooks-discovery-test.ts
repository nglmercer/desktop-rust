// @vitest-environment node
import { execFileSync } from 'node:child_process'
import {
  chmodSync,
  mkdtempSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

import {
  getRepoHooks,
  getRepoHooksPath,
} from '../../src/lib/hooks/get-repo-hooks'
import { quoteGitConfigParameter } from '../../src/lib/hooks/git-config-parameters'

describe.skipIf(process.platform === 'win32')('Git hook discovery', () => {
  let root: string | undefined
  afterEach(() => {
    if (root !== undefined) rmSync(root, { recursive: true, force: true })
    root = undefined
  })

  it('resolves configured relative hook paths and honors the last value', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hooks-config-'))
    execFileSync('git', ['init', '-q', root])
    const hooks = join(root, 'custom-hooks')
    mkdirSync(hooks)
    writeFileSync(join(hooks, 'pre-commit'), '#!/bin/sh\nexit 0\n')
    chmodSync(join(hooks, 'pre-commit'), 0o755)
    execFileSync('git', ['config', '--add', 'core.hooksPath', 'missing'], {
      cwd: root,
    })
    execFileSync('git', ['config', '--add', 'core.hooksPath', 'custom-hooks'], {
      cwd: root,
    })
    expect(await getRepoHooksPath(root)).toBe(hooks)
    expect(await Array.fromAsync(getRepoHooks(root))).toEqual(['pre-commit'])
  })

  it('preserves an empty configured hooks path as the repository root', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hooks-empty-'))
    execFileSync('git', ['init', '-q', root])
    execFileSync('git', ['config', 'core.hooksPath', ''], { cwd: root })
    expect(await getRepoHooksPath(root)).toBe(root)
    expect(await Array.fromAsync(getRepoHooks(root))).toEqual([])
  })

  it('uses hook path configuration supplied through the operation environment', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hooks-env-'))
    execFileSync('git', ['init', '-q', root])
    const hooks = join(root, 'environment-hooks')
    mkdirSync(hooks)
    writeFileSync(join(hooks, 'commit-msg'), '#!/bin/sh\nexit 0\n')
    chmodSync(join(hooks, 'commit-msg'), 0o755)
    const env = {
      GIT_CONFIG_PARAMETERS: quoteGitConfigParameter(`core.hooksPath=${hooks}`),
    }
    expect(await getRepoHooksPath(root, env)).toBe(hooks)
    expect(await Array.fromAsync(getRepoHooks(root, undefined, env))).toEqual([
      'commit-msg',
    ])
  })

  it('skips non-executable files, unknown names, and empty filters', async () => {
    root = mkdtempSync(join(tmpdir(), 'desktop-hooks-filter-'))
    execFileSync('git', ['init', '-q', root])
    const hooks = join(root, '.git', 'hooks')
    writeFileSync(join(hooks, 'pre-commit'), '#!/bin/sh\nexit 0\n')
    chmodSync(join(hooks, 'pre-commit'), 0o644)
    writeFileSync(join(hooks, 'not-a-hook'), '#!/bin/sh\nexit 0\n')
    chmodSync(join(hooks, 'not-a-hook'), 0o755)
    writeFileSync(join(hooks, 'post-merge.exe'), '#!/bin/sh\nexit 0\n')
    chmodSync(join(hooks, 'post-merge.exe'), 0o755)
    expect(await Array.fromAsync(getRepoHooks(root, []))).toEqual([])
    expect(await Array.fromAsync(getRepoHooks(root))).toEqual(['post-merge'])
  })

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
    expect(
      await Array.fromAsync(
        getRepoHooks(root, ['pre-commit', 'commit-msg', 'post-commit'])
      )
    ).toEqual(['pre-commit'])
    expect(await Array.fromAsync(getRepoHooks(root, ['*']))).toEqual([
      'pre-commit',
    ])
  })
})
