// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createContext, runInContext } from 'node:vm'
import { describe, it } from 'vitest'
import { findTestFilesIn } from './test-discovery.mjs'

function launchInContext(
  source: string,
  platform: 'darwin' | 'win32',
  args: string[]
) {
  const paths = platform === 'darwin' ? path.posix : path.win32
  const directory =
    platform === 'darwin'
      ? '/Applications/GitHub Desktop.app/Contents/Resources/app'
      : 'C:\\Apps\\GitHubDesktop\\resources\\app'
  const calls: Array<{ command: string; args: string[]; options?: unknown }> =
    []
  const environment: Record<string, string> = { ELECTRON_RUN_AS_NODE: '1' }
  const childProcess = {
    execFile(command: string, argv: string[]) {
      calls.push({ command, args: argv })
    },
    spawn(command: string, argv: string[], options: unknown) {
      calls.push({ command, args: argv, options })
      const child = {
        on() {
          return child
        },
        unref() {},
      }
      return child
    },
  }
  const context = createContext({
    __dirname: directory,
    console,
    exports: {},
    module: { exports: {} },
    process: { argv: ['node', 'cli.js', ...args], platform, env: environment },
    require(id: string) {
      if (id === 'path') return paths
      if (id === 'child_process') return childProcess
      throw new Error(`Unexpected external dependency ${id}`)
    },
  })
  runInContext(source, context, { timeout: 5000 })
  assert.equal(environment.ELECTRON_RUN_AS_NODE, undefined)
  return calls
}

describe('Vite Electron CLI build', () => {
  it('discovers dotted and hyphenated test filenames in a directory', async () => {
    const directory = mkdtempSync(
      path.join(tmpdir(), 'desktop-test-discovery-')
    )
    try {
      writeFileSync(
        path.join(directory, 'sample.test.ts'),
        "import { test } from 'node:test'\ntest('dotted discovery works', () => {})\n"
      )
      writeFileSync(path.join(directory, 'other-test.ts'), '')
      writeFileSync(path.join(directory, 'unrelated.ts'), '')
      assert.deepEqual(
        (await findTestFilesIn([directory])).sort(),
        [
          path.join(directory, 'other-test.ts'),
          path.join(directory, 'sample.test.ts'),
        ].sort()
      )
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })

  for (const { channel, nodeEnv, isDevelopment } of [
    { channel: 'development', nodeEnv: 'development', isDevelopment: true },
    { channel: 'production', nodeEnv: 'production', isDevelopment: false },
    { channel: 'development', nodeEnv: 'production', isDevelopment: true },
    { channel: 'beta', nodeEnv: 'development', isDevelopment: false },
  ] as const) {
    it(`preserves launcher behavior for ${channel} with NODE_ENV=${nodeEnv}`, () => {
      const output = mkdtempSync(path.join(tmpdir(), 'desktop-cli-build-'))
      const sentinel = path.join(output, 'other-bundle.js')
      writeFileSync(sentinel, 'preserve')
      try {
        const built = spawnSync(
          process.execPath,
          [
            'node_modules/vite/bin/vite.js',
            'build',
            '-c',
            'vite.cli.config.ts',
            '--outDir',
            output,
          ],
          {
            encoding: 'utf8',
            timeout: 60_000,
            env: {
              ...process.env,
              NODE_ENV: nodeEnv,
              RELEASE_CHANNEL: channel,
            },
          }
        )
        assert.equal(built.status, 0, built.stderr)
        assert.equal(readFileSync(sentinel, 'utf8'), 'preserve')
        assert.ok(readFileSync(path.join(output, 'cli.js.map')).length > 0)
        const source = readFileSync(path.join(output, 'cli.js'), 'utf8')
        for (const args of [['--help'], ['clone']]) {
          const reference = spawnSync(
            process.execPath,
            ['--import', 'tsx', 'app/src/cli/main.ts', ...args],
            { encoding: 'utf8' }
          )
          const result = spawnSync(
            process.execPath,
            [path.join(output, 'cli.js'), ...args],
            { encoding: 'utf8' }
          )
          assert.equal(result.status, reference.status)
          assert.equal(result.stderr, reference.stderr)
          assert.equal(result.stdout, reference.stdout)
        }

        const mac = launchInContext(source, 'darwin', [
          'clone',
          '-b',
          'feature',
          'alice/project',
        ])
        assert.equal(mac.length, 1)
        assert.equal(mac[0].command, 'open')
        assert.deepEqual(Array.from(mac[0].args), [
          '-n',
          '/Applications/GitHub Desktop.app',
          '--args',
          '--cli-clone=https://github.com/alice/project',
          '--cli-branch=feature',
        ])

        const windows = launchInContext(source, 'win32', [
          'open',
          'C:\\Work\\project',
        ])
        assert.equal(windows.length, 1)
        assert.equal(
          windows[0].command,
          `C:\\Apps\\GitHubDesktop\\GitHubDesktop${isDevelopment ? '-dev' : ''}.exe`
        )
        assert.deepEqual(Array.from(windows[0].args), [
          '--cli-open=C:\\Work\\project',
        ])
        assert.deepEqual(JSON.parse(JSON.stringify(windows[0].options)), {
          detached: true,
          stdio: 'ignore',
        })
      } finally {
        rmSync(output, { recursive: true, force: true })
      }
    }, 90_000)
  }
})
