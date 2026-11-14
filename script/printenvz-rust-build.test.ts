// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  cpSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { afterEach, describe, it } from 'vitest'
import { resolveRustTarget } from './printenvz'

const directories: string[] = []
afterEach(() => {
  for (const directory of directories.splice(0)) {
    rmSync(directory, { recursive: true, force: true })
  }
})

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'printenvz build '))
  directories.push(root)
  const driver = path.join(root, 'build.ts')
  writeFileSync(driver, `import { buildPrintenvz } from ${JSON.stringify(path.resolve('script/printenvz.ts'))}\nbuildPrintenvz(${JSON.stringify(root)})\n`)
  const rustc = path.join(root, 'fake rustc')
  const cargo = path.join(root, 'fake cargo')
  const capture = path.join(root, 'cargo-args')
  writeFileSync(
    rustc,
    '#!/bin/sh\nprintf "host: x86_64-unknown-linux-gnu\\n"\n'
  )
  writeFileSync(
    cargo,
    '#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\nexit "${FAKE_CARGO_STATUS:-0}"\n'
  )
  chmodSync(rustc, 0o755)
  chmodSync(cargo, 0o755)
  const output = path.join(root, 'build/Release/printenvz')
  mkdirSync(path.dirname(output), { recursive: true })
  writeFileSync(output, 'previous binary')
  const targetBinary = path.join(
    root,
    'build/cargo/aarch64-unknown-linux-gnu/release/printenvz'
  )
  mkdirSync(path.dirname(targetBinary), { recursive: true })
  writeFileSync(targetBinary, 'arm64 binary')
  const invoke = (extra: NodeJS.ProcessEnv = {}) =>
    spawnSync(process.execPath, ['--import', require.resolve('tsx'), driver], {
      cwd: root,
      env: {
        ...process.env,
        RUSTC: rustc,
        CARGO: cargo,
        CAPTURE: capture,
        npm_config_cpu: 'arm64',
        npm_config_arch: '',
        npm_config_os: process.platform,
        TARGET_ARCH: '',
        CARGO_BUILD_TARGET: 'x86_64-unknown-linux-gnu',
        ...extra,
      },
      encoding: 'utf8',
      timeout: 10_000,
    })
  return { output, capture, invoke }
}

describe.skipIf(process.platform !== 'linux')(
  'Rust printenvz installer',
  () => {
    it('builds and copies the requested architecture, even when Cargo has another default', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke()
      assert.equal(result.status, 0, result.stderr)
      assert.match(
        readFileSync(capture, 'utf8'),
        /--target\naarch64-unknown-linux-gnu\n/
      )
      assert.equal(readFileSync(output, 'utf8'), 'arm64 binary')
    })

    it('preserves the previous executable if the requested target build fails', () => {
      const { output, invoke } = fixture()
      const result = invoke({ FAKE_CARGO_STATUS: '101' })
      assert.notEqual(result.status, 0)
      assert.equal(readFileSync(output, 'utf8'), 'previous binary')
    })

    it('rejects an unknown architecture before invoking Cargo', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke({ npm_config_cpu: 'not-a-real-cpu' })
      assert.notEqual(result.status, 0)
      assert.match(result.stderr, /Unsupported printenvz cross target/)
      assert.equal(existsSync(capture), false)
      assert.equal(readFileSync(output, 'utf8'), 'previous binary')
    })

    describe.skipIf(process.platform !== 'linux')('Copied Rust helper', () => {
      it('builds the copied source explicitly and loads it through getShellEnv', async () => {
        const root = mkdtempSync(path.join(tmpdir(), "printenvz installed ' "))
        directories.push(root)
        const source = path.resolve('vendor/printenvz')
        copyFileSync(path.join(source, 'Cargo.toml'), path.join(root, 'Cargo.toml'))
        copyFileSync(path.join(source, 'Cargo.lock'), path.join(root, 'Cargo.lock'))
        cpSync(path.join(source, 'src'), path.join(root, 'src'), { recursive: true })
        const env = { ...process.env }
        delete env.TARGET_ARCH
        for (const key of Object.keys(env)) {
          if (/^npm_config_(?:cpu|arch|os)$/i.test(key)) {
            delete env[key]
          }
        }
        const binary = path.join(root, 'build/Release/printenvz')
        assert.equal(existsSync(binary), false)
        const driver = path.join(root, 'build.ts')
        writeFileSync(driver, `import { buildPrintenvz } from ${JSON.stringify(path.resolve('script/printenvz.ts'))}\nbuildPrintenvz(${JSON.stringify(root)})\n`)
        const build = spawnSync(
          process.execPath,
          ['--import', require.resolve('tsx'), driver],
          {
            cwd: root,
            env,
            encoding: 'utf8',
            timeout: 60_000,
          }
        )
        assert.equal(build.status, 0, build.stderr)
        const output = spawnSync(binary, [], {
          env: { EMPTY: '', TEXT: 'hello ☃' },
        })
        assert.equal(output.status, 0)
        assert.ok(output.stdout.includes(Buffer.from('EMPTY=\0')))
        assert.ok(output.stdout.includes(Buffer.from('TEXT=hello ☃\0')))

        // Exercise the actual loader and shell command quoting without sourcing
        // the user's login files. The fixture shell exports one known value.
        const shell = path.join(root, "shell with ' spaces")
        writeFileSync(
          shell,
          '#!/bin/sh\n[ "$1" = "-ilc" ] || exit 2\nexport PRINTENVZ_FIXTURE=loaded\nexec /bin/sh -c "$2"\n'
        )
        chmodSync(shell, 0o755)
        const previous = process.env.SHELL
        try {
          process.env.SHELL = shell
          const { getShellEnv } = await import(
            '../app/src/lib/hooks/get-shell-env'
          )
          const result = await getShellEnv(root, undefined, binary)
          assert.equal(result.kind, 'success')
          if (result.kind === 'success') {
            assert.equal(result.env.PRINTENVZ_FIXTURE, 'loaded')
          }
        } finally {
          if (previous === undefined) {
            delete process.env.SHELL
          } else {
            process.env.SHELL = previous
          }
        }
      }, 90_000)
    })

    it('rejects conflicting npm CPU settings without replacing the old binary', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke({ npm_config_cpu: 'arm64', npm_config_arch: 'x64' })
      assert.notEqual(result.status, 0)
      assert.match(result.stderr, /Conflicting printenvz CPU settings/)
      assert.equal(existsSync(capture), false)
      assert.equal(readFileSync(output, 'utf8'), 'previous binary')
    })

    it('uses the packaging TARGET_ARCH when npm did not specify a CPU', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke({ npm_config_cpu: '', TARGET_ARCH: 'arm64' })
      assert.equal(result.status, 0, result.stderr)
      assert.match(
        readFileSync(capture, 'utf8'),
        /--target\naarch64-unknown-linux-gnu\n/
      )
      assert.equal(readFileSync(output, 'utf8'), 'arm64 binary')
    })

    it('rejects a packaging target that disagrees with npm', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke({ TARGET_ARCH: 'x64' })
      assert.notEqual(result.status, 0)
      assert.match(result.stderr, /Conflicting printenvz CPU settings/)
      assert.equal(existsSync(capture), false)
      assert.equal(readFileSync(output, 'utf8'), 'previous binary')
    })

    it('rejects an OS different from the current host', () => {
      const { output, capture, invoke } = fixture()
      const result = invoke({ npm_config_os: 'win32' })
      assert.notEqual(result.status, 0)
      assert.match(result.stderr, /Unsupported printenvz target OS/)
      assert.equal(existsSync(capture), false)
      assert.equal(readFileSync(output, 'utf8'), 'previous binary')
    })
  }
)

describe('Rust target selection', () => {
  it('preserves native ARM ABI and rejects unsafe substitutions', () => {
    assert.equal(
      resolveRustTarget('arm-unknown-linux-gnueabihf', 'linux', 'arm'),
      'arm-unknown-linux-gnueabihf'
    )
    assert.throws(
      () => resolveRustTarget('arm-unknown-linux-gnueabihf', 'linux', 'x64'),
      /Unsupported printenvz cross target/
    )
  })

  it('selects explicit supported macOS, Windows and Android cross targets', () => {
    assert.equal(
      resolveRustTarget('aarch64-apple-darwin', 'darwin', 'x64'),
      'x86_64-apple-darwin'
    )
    assert.equal(
      resolveRustTarget('x86_64-pc-windows-msvc', 'win32', 'arm64'),
      'aarch64-pc-windows-msvc'
    )
    assert.equal(
      resolveRustTarget('aarch64-linux-android', 'android', 'arm'),
      'armv7-linux-androideabi'
    )
  })
})
