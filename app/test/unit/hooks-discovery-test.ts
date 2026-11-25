// @vitest-environment node
import { execFileSync } from 'node:child_process'
import {
  access as accessAsync,
  readdir as readdirAsync,
  stat as statAsync,
} from 'node:fs/promises'
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
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

function fsBridge(operation: string, options: any): Promise<unknown> {
  const metadata = (value: {
    isFile(): boolean
    isDirectory(): boolean
    isSymbolicLink(): boolean
  }) => ({
    isFile: value.isFile(),
    isDirectory: value.isDirectory(),
    isSymbolicLink: value.isSymbolicLink(),
  })
  switch (operation) {
    case 'access':
      return accessAsync(options.path, options.mode).then(() => null)
    case 'stat':
      return statAsync(options.path).then(value => ({
        ...metadata(value),
        size: value.size,
        mode: value.mode,
        dev: value.dev,
        ino: String(value.ino),
        mtimeMs: value.mtimeMs,
        atimeMs: value.atimeMs,
        birthtimeMs: value.birthtimeMs,
        ctimeMs: value.ctimeMs,
      }))
    case 'readdir':
      return readdirAsync(options.path, { withFileTypes: true }).then(entries =>
        entries.map(entry => ({ name: entry.name, ...metadata(entry) }))
      )
    default:
      return Promise.reject(new Error(`unexpected fs ${operation}`))
  }
}

function installGitBridge() {
  vi.stubGlobal('__desktop', {
    invoke: (channel: string, options: any): Promise<unknown> => {
      if (channel.startsWith('desktop.io.fs-')) {
        return fsBridge(channel.slice('desktop.io.fs-'.length), options)
      }
      if (channel !== 'desktop.io.git-exec') {
        return Promise.reject(new Error(`unexpected ${channel}`))
      }
      try {
        return Promise.resolve({
          exitCode: 0,
          stdout: execFileSync('git', options.args, {
            cwd: options.cwd,
            encoding: 'utf8',
          }),
          stderr: '',
        })
      } catch (error) {
        const result = error as {
          status?: number
          stdout?: string
          stderr?: string
        }
        return Promise.resolve({
          exitCode: result.status ?? 1,
          stdout: result.stdout ?? '',
          stderr: result.stderr ?? '',
        })
      }
    },
    invokeSync: () => {
      throw new Error('unexpected synchronous call')
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  })
}

import { getRepoHooks } from '../../src/lib/hooks/get-repo-hooks'

describe.skipIf(process.platform === 'win32')('Git hook discovery', () => {
  let root: string | undefined
  beforeEach(() => {
    installGitBridge()
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    if (root !== undefined) rmSync(root, { recursive: true, force: true })
    root = undefined
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
