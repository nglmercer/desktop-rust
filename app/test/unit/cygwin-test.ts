import assert from 'node:assert'
import childProcess, { ChildProcess } from 'child_process'
import { mkdir, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { afterEach, describe, it, vi } from 'vitest'
import { promisify } from 'node:util'

import { launch, Shell } from '../../src/lib/shells/win32'
import { createTempDirectory } from '../helpers/temp'

vi.mock('child_process', async importOriginal => {
  const actual = await importOriginal<typeof import('child_process')>()
  const spawn = vi.fn()
  return { ...actual, spawn, default: { ...actual, spawn } }
})

const execFile = promisify(childProcess.execFile)

describe('Cygwin launch', () => {
  afterEach(() => vi.mocked(childProcess.spawn).mockReset())
  const shellPath = 'C:\\Program Files\\Cygwin\\bin\\mintty.exe'
  const paths = [
    'C:\\repositories\\project',
    'C:\\repositories\\my project',
    "C:\\repositories\\someone's project",
    'C:\\repositories\\caf\u00e9',
    'C:\\repositories\\project$(name)',
    'C:\\repositories\\project`name`',
    'C:\\repositories\\project${name}',
    'C:\\repositories\\project%name%',
    'C:\\repositories\\project&notes',
    '\\\\server\\share\\project',
  ]

  for (const path of paths) {
    it(`passes the repository path through the environment for ${path}`, () => {
      const child = new ChildProcess()
      const spawn = vi.mocked(childProcess.spawn).mockImplementation(() => child)

      assert.strictEqual(
        launch({ shell: Shell.Cygwin, path: shellPath }, path),
        child
      )
      assert.strictEqual(spawn.mock.calls.length, 1)
      const [executable, args, options] = spawn.mock.calls[0]
      assert.strictEqual(executable, shellPath)
      assert.deepStrictEqual(args, [
        '/bin/sh',
        '-lc',
        'cd -- "$(cygpath -- "$GITHUB_DESKTOP_CYGWIN_OPEN_PATH")" && exec bash',
      ])
      assert.strictEqual(options?.shell, undefined)
      assert.strictEqual(options?.cwd, path)
      assert.strictEqual(options?.env?.GITHUB_DESKTOP_CYGWIN_OPEN_PATH, path)
      assert.ok(
        Object.entries(process.env).every(
          ([key, value]) =>
            key === 'GITHUB_DESKTOP_CYGWIN_OPEN_PATH' ||
            options?.env?.[key] === value
        ),
        'The terminal should inherit the existing environment'
      )
    })
  }

  it.skipIf(__WIN32__)(
    'opens the requested directory and stops if it is unavailable',
    async t => {
      const root = await createTempDirectory(t)
      const bin = join(root, 'bin')
      await mkdir(bin)
      await writeFile(
        join(bin, 'cygpath'),
        '#!/bin/sh\n[ "$#" -eq 2 ] && [ "$1" = "--" ] || exit 1\nprintf "%s" "$2"\n',
        { mode: 0o755 }
      )
      await writeFile(join(bin, 'bash'), '#!/bin/sh\npwd -P\n', { mode: 0o755 })

      const spawn = vi.mocked(childProcess.spawn).mockImplementation(() => new ChildProcess())

      for (const path of paths) {
        const directory = join(root, path)
        await mkdir(directory)
        launch({ shell: Shell.Cygwin, path: shellPath }, directory)
        const [, args, options] =
          spawn.mock.calls[spawn.mock.calls.length - 1]
        assert.ok(args)
        assert.ok(options?.env)
        // Stand-ins isolate the shell script from Cygwin and login profiles.
        const { stdout } = await execFile('/bin/sh', ['-c', args[2]], {
          cwd: root,
          env: { ...options.env, PATH: bin },
        })
        assert.strictEqual(stdout.trimEnd(), await realpath(directory))
      }

      launch({ shell: Shell.Cygwin, path: shellPath }, join(root, 'missing'))
      const [, args, options] =
        spawn.mock.calls[spawn.mock.calls.length - 1]
      assert.ok(args)
      assert.ok(options?.env)
      await assert.rejects(
        execFile('/bin/sh', ['-c', args[2]], {
          cwd: root,
          env: { ...options.env, PATH: bin },
        }),
        { stdout: '' }
      )
    }
  )

  it('propagates errors when starting the terminal', () => {
    const error = new Error('Unable to start terminal')
    vi.mocked(childProcess.spawn).mockImplementation(() => {
      throw error
    })

    assert.throws(
      () => launch({ shell: Shell.Cygwin, path: shellPath }, paths[0]),
      error
    )
  })
})
