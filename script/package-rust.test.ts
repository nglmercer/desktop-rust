import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  mkdirSync,
  mkdtempSync,
  cpSync,
  existsSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

const spawn = vi.hoisted(() =>
  vi.fn((..._args: unknown[]) => ({ status: 0, stdout: '', stderr: '' }))
)
vi.mock('node:child_process', () => ({
  spawnSync: spawn,
  default: { spawnSync: spawn },
}))
import {
  acquirePackageOutputLock,
  copilotPlatform,
  packageRust,
  stagedRustDesktop,
} from './package-rust'
import { acquireRustPackageLock } from './rust-package-lock'

const roots: string[] = []
const host =
  process.platform === 'darwin'
    ? 'macos'
    : process.platform === 'win32'
    ? 'windows'
    : 'linux'
const suffix = process.platform === 'win32' ? '.exe' : ''
const report = process.report?.getReport() as
  | { header?: { glibcVersionRuntime?: string } }
  | undefined
const linuxMusl =
  host === 'linux' &&
  report !== undefined &&
  !report.header?.glibcVersionRuntime
const sdkPlatform = copilotPlatform(host, linuxMusl)

function write(root: string, file: string, content = 'test'): void {
  const full = path.join(root, file)
  mkdirSync(path.dirname(full), { recursive: true })
  writeFileSync(full, content)
}

function fixture(): string {
  const root = mkdtempSync(path.join(os.tmpdir(), 'package-rust-test-'))
  roots.push(root)
  write(
    root,
    'app/package.json',
    JSON.stringify({
      productName: 'Test Desktop',
      version: '1.2.3',
      bundleID: 'org.test',
      dependencies: { '@github/copilot-sdk': '1.2.3' },
    })
  )
  for (const file of [
    'node_modules/vite/bin/vite.js',
    `rust/target/desktop-packaged/debug/desktop-rs${suffix}`,
    `rust/target/desktop-packaged/debug/github-desktop-rs${suffix}`,
    `rust/target/desktop-packaged/release/desktop-rs${suffix}`,
    `rust/target/desktop-packaged/release/github-desktop-rs${suffix}`,
    'out/vite-dist/index.html',
    'out/vite-dist/highlighter.js',
    'out/vite-dist/ipc-shim.js',
    'out/vite-dist/static/licenses.json',
    'out/vite-dist/static/LICENSE.choosealicense.md',
    'gemoji/db/emoji.json',
    'gemoji/LICENSE',
    'LICENSE',
    'rust/THIRD_PARTY_NOTICES.txt',
  ])
    write(root, file)
  write(
    root,
    'rust/crates/core/src/copilot.rs',
    'pub const COPILOT_SDK_VERSION: &str = "1.2.3";'
  )
  write(root, 'rust/crates/desktop-app/Cargo.toml')
  write(root, 'rust/crates/cli/Cargo.toml')
  write(root, 'out/vite-dist/assets/renderer.js', 'renderer')
  write(
    root,
    'out/vite-dist/static/licenses.json',
    JSON.stringify({ desktop: { license: 'MIT' } })
  )
  return root
}

beforeEach(() => {
  vi.stubEnv('NODE_ENV', undefined)
  vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', 'debug')
  vi.stubEnv('DESKTOP_RUST_DIST_DIR', '')
  spawn.mockImplementation((...args: unknown[]) => {
    const [command, values, options] = args as [
      string,
      string[],
      { cwd: string; env?: NodeJS.ProcessEnv }
    ]
    if (command === 'rustc') {
      const rustArch = process.arch === 'x64' ? 'x86_64' : 'aarch64'
      const rustOs =
        host === 'macos'
          ? 'apple-darwin'
          : host === 'windows'
          ? 'pc-windows-msvc'
          : linuxMusl
          ? 'unknown-linux-musl'
          : 'unknown-linux-gnu'
      return { status: 0, stdout: `host: ${rustArch}-${rustOs}\n`, stderr: '' }
    }
    if (command === 'cargo') {
      if (values[0] === 'metadata') {
        if (process.env.FAKE_METADATA_ERROR)
          return { status: 1, stdout: '', stderr: 'unavailable' }
        const packages = [
          {
            id: 'desktop-app',
            name: 'desktop-app',
            manifest_path: path.join(
              options.cwd,
              'rust/crates/desktop-app/Cargo.toml'
            ),
          },
          {
            id: 'desktop-cli',
            name: 'desktop-cli',
            manifest_path: path.join(options.cwd, 'rust/crates/cli/Cargo.toml'),
          },
        ].map(pkg => ({
          ...pkg,
          version: '1.0.0',
          source: null,
          repository: null,
          license: 'MIT',
          license_file: null,
        }))
        return {
          status: 0,
          stdout: JSON.stringify({
            packages,
            workspace_members: packages.map(pkg => pkg.id),
            resolve: { nodes: packages.map(pkg => ({ id: pkg.id, deps: [] })) },
          }),
          stderr: '',
        }
      }
      const name = values.includes('desktop-cli')
        ? 'desktop-rs'
        : 'github-desktop-rs'
      const profile = values.includes('--release') ? 'release' : 'debug'
      const directory = `rust/target/desktop-packaged/${
        process.env.FAKE_BAD_ARTIFACT
          ? profile === 'debug'
            ? 'release'
            : 'debug'
          : profile
      }`
      const executable = path.join(options.cwd, directory, `${name}${suffix}`)
      return {
        status: 0,
        stdout: JSON.stringify({
          reason: 'compiler-artifact',
          target: { name },
          executable,
        }),
        stderr: '',
      }
    }
    if (
      command === process.execPath &&
      values[0]?.endsWith('vite.js') &&
      values.length === 2
    ) {
      const output = options.env?.DESKTOP_VITE_DIST
      if (!output) throw new Error('Missing isolated Vite output')
      cpSync(path.join(options.cwd, 'out/vite-dist'), output, {
        recursive: true,
      })
    }
    return { status: 0, stdout: '', stderr: '' }
  })
})

afterEach(() => {
  for (const root of roots.splice(0))
    rmSync(root, { recursive: true, force: true })
  vi.clearAllMocks()
  vi.unstubAllEnvs()
})

describe('Rust packaging', () => {
  it.each([
    ['production', 'debug'],
    ['development', 'release'],
  ])(
    'requires production NODE_ENV for %s channel and %s profile',
    (channel, profile) => {
      const root = fixture()
      vi.stubEnv('RELEASE_CHANNEL', channel)
      vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', profile)
      vi.stubEnv('NODE_ENV', 'development')
      expect(() => packageRust(root)).toThrow(
        'Production packages require NODE_ENV=production'
      )
      expect(spawn).not.toHaveBeenCalled()
    }
  )

  it('sets a consistent production environment for Vite and Cargo', () => {
    const root = fixture()
    vi.stubEnv('RELEASE_CHANNEL', 'production')
    vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', 'release')
    packageRust(root)
    const builds = spawn.mock.calls.filter(
      ([command, values]) =>
        command === process.execPath ||
        (command === 'cargo' && (values as string[])[0] === 'build')
    )
    expect(builds).toHaveLength(5)
    for (const call of builds) {
      const options = call[2] as { env: NodeJS.ProcessEnv }
      expect(options.env.NODE_ENV).toBe('production')
      expect(options.env.RELEASE_CHANNEL).toBe('production')
    }
  })
  it('blocks a destination even when Cargo targets differ and preserves its lock', () => {
    const root = fixture()
    const release = acquirePackageOutputLock(root, 'development')
    vi.stubEnv('CARGO_TARGET_DIR', path.join(root, 'other-target'))
    expect(() => packageRust(root)).toThrow('lock was retained')
    release()
    const reacquired = acquirePackageOutputLock(root, 'development')
    reacquired()
  })

  it('blocks a target shared by different destinations and releases its own destination lock', () => {
    const root = fixture()
    const target = path.join(root, 'rust/target/desktop-packaged')
    const release = acquireRustPackageLock(
      path.join(target, '.desktop-package-target.lock')
    )
    vi.stubEnv('DESKTOP_RUST_DIST_DIR', path.join(root, 'other-dist'))
    expect(() => packageRust(root)).toThrow('lock was retained')
    const destination = acquirePackageOutputLock(root, 'development')
    destination()
    release()
  })

  it('does not remove a replaced or preexisting lock', () => {
    const root = fixture()
    const lock = path.join(root, 'test.lock')
    const release = acquireRustPackageLock(lock)
    expect(() => acquireRustPackageLock(lock)).toThrow('lock was retained')
    writeFileSync(lock, 'another owner')
    expect(release).toThrow('lock changed')
    expect(readFileSync(lock, 'utf8')).toBe('another owner')
  })
  it('stages into an explicit absolute output directory', () => {
    const root = fixture()
    const destination = path.join(root, 'custom destination')
    vi.stubEnv('DESKTOP_RUST_DIST_DIR', destination)
    const output = packageRust(root)
    expect(path.dirname(output)).toBe(destination)
    expect(
      readFileSync(path.join(output, 'ui/assets/renderer.js'), 'utf8')
    ).toBe('renderer')
    expect(
      stagedRustDesktop(root, { channel: 'development', profile: 'debug' })
    ).toBe(path.join(output, 'bin', `github-desktop-rs${suffix}`))
  })

  it('requires a staged package before starting the desktop', () => {
    const root = fixture()
    expect(() =>
      stagedRustDesktop(root, { channel: 'production', profile: 'release' })
    ).toThrow('run pnpm package first')
  })

  it.skipIf(process.platform === 'win32')(
    'refuses a symlinked staged desktop executable',
    () => {
      const root = fixture()
      const output = packageRust(root)
      const executable = path.join(output, 'bin', 'github-desktop-rs')
      rmSync(executable)
      symlinkSync(path.join(root, 'LICENSE'), executable)
      expect(() =>
        stagedRustDesktop(root, { channel: 'development', profile: 'debug' })
      ).toThrow('run pnpm package first')
    }
  )

  it('refuses a debug package when a release desktop is requested', () => {
    const root = fixture()
    vi.stubEnv('RELEASE_CHANNEL', 'production')
    packageRust(root)
    expect(() =>
      stagedRustDesktop(root, { channel: 'production', profile: 'release' })
    ).toThrow('run pnpm package first')
  })

  it('rejects relative output directories before running build commands', () => {
    const root = fixture()
    vi.stubEnv('DESKTOP_RUST_DIST_DIR', 'relative-output')
    expect(() => packageRust(root)).toThrow(
      'DESKTOP_RUST_DIST_DIR must be an absolute path'
    )
    expect(spawn).not.toHaveBeenCalled()
  })

  it('does not replace an unrelated directory at the package destination', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    const root = fixture()
    const destination = path.join(root, 'custom destination')
    vi.stubEnv('DESKTOP_RUST_DIST_DIR', destination)
    const unrelated = path.join(
      destination,
      `Test Desktop-dev-${host}-${process.arch}`,
      'important.txt'
    )
    mkdirSync(path.dirname(unrelated), { recursive: true })
    writeFileSync(unrelated, 'keep')
    expect(() => packageRust(root)).toThrow(
      'Refusing to replace an unrecognized package directory'
    )
    expect(readFileSync(unrelated, 'utf8')).toBe('keep')
  })

  it('replaces a previously staged Rust package', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    const root = fixture()
    const output = packageRust(root)
    writeFileSync(path.join(output, 'meta', 'old-file'), 'old')
    expect(packageRust(root)).toBe(output)
    expect(() => readFileSync(path.join(output, 'meta', 'old-file'))).toThrow()
  })

  it('builds and stages the actual shell and embedded browser UI', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const sdk = `app/node_modules/@github/copilot-sdk-${sdkPlatform}-${process.arch}/prebuilds/${sdkPlatform}-${process.arch}`
    write(root, `${sdk}/copilot-runtime${suffix}`, 'runtime')
    write(root, `${sdk}/runtime.node`, 'addon')
    write(
      root,
      `app/node_modules/@github/copilot-sdk-${sdkPlatform}-${process.arch}/package.json`,
      JSON.stringify({
        name: `@github/copilot-sdk-${sdkPlatform}-${process.arch}`,
        version: '1.2.3',
      })
    )
    const output = packageRust(root)
    const bundleArguments = spawn.mock.calls
      .filter(
        call =>
          call[0] === process.execPath ||
          (call[0] === 'cargo' && (call[1] as string[])[0] === 'build')
      )
      .map(
        call => (call[2] as { env?: NodeJS.ProcessEnv }).env?.DESKTOP_VITE_DIST
      )
    expect(bundleArguments.length).toBe(5)
    expect(new Set(bundleArguments).size).toBe(1)
    const privateBundle = bundleArguments[0]
    expect(privateBundle).toBeDefined()
    expect(privateBundle).not.toBe(path.join(root, 'out/vite-dist'))
    expect(privateBundle && existsSync(privateBundle)).toBe(false)
    expect(
      readFileSync(path.join(root, 'out/vite-dist/index.html'), 'utf8')
    ).toBe('test')
    expect(
      readFileSync(path.join(output, 'ui/assets/renderer.js'), 'utf8')
    ).toBe('renderer')
    expect(
      readFileSync(
        path.join(
          output,
          'bin/copilot-sdk/prebuilds',
          `${sdkPlatform}-${process.arch}`,
          `copilot-runtime${suffix}`
        ),
        'utf8'
      )
    ).toBe('runtime')
    expect(
      JSON.parse(readFileSync(path.join(output, 'meta/package.json'), 'utf8'))
    ).toMatchObject({
      platform: host,
      profile: 'debug',
      executable: `bin/desktop-rs${suffix}`,
    })
    expect(spawn.mock.calls.map(call => call[1])).toEqual([
      ['-vV'],
      [path.join(root, 'node_modules/vite/bin/vite.js'), 'build'],
      [
        path.join(root, 'node_modules/vite/bin/vite.js'),
        'build',
        '-c',
        'vite.highlighter.config.ts',
      ],
      [
        path.join(root, 'node_modules/vite/bin/vite.js'),
        'build',
        '-c',
        'vite.ipc-shim.config.ts',
      ],
      [
        'build',
        '--locked',
        '-p',
        'desktop-cli',
        '--features',
        'embedded-ui,packaged-runtime',
        '--message-format=json-render-diagnostics',
      ],
      [
        'build',
        '--locked',
        '-p',
        'desktop-app',
        '--features',
        'packaged-ui',
        '--message-format=json-render-diagnostics',
      ],
      [
        'metadata',
        '--locked',
        '--format-version=1',
        '--filter-platform',
        expect.any(String),
        '--features',
        'desktop-cli/embedded-ui,desktop-cli/packaged-runtime,desktop-app/packaged-ui',
      ],
    ])
    const inventory = JSON.parse(
      readFileSync(path.join(output, 'meta/rust-dependencies.json'), 'utf8')
    )
    expect(inventory.packages).toHaveLength(2)
    expect(
      inventory.packages.every(
        (pkg: { noLocalNoticeFound: boolean }) => pkg.noLocalNoticeFound
      )
    ).toBe(true)
    for (const file of [
      'LICENSE',
      'THIRD_PARTY_NOTICES.txt',
      'gemoji-LICENSE',
      'web-notices.json',
      'choosealicense-LICENSE',
    ]) {
      expect(
        readFileSync(path.join(output, 'licenses', file)).length
      ).toBeGreaterThan(0)
    }
  })

  it('rejects an unrelated target before building', () => {
    const root = fixture()
    const other = host === 'linux' ? 'windows' : 'linux'
    expect(() => packageRust(root, other)).toThrow(
      'build on the target platform'
    )
    expect(spawn).not.toHaveBeenCalled()
  })

  it('uses the SDK platform names expected by the Rust runtime', () => {
    expect(copilotPlatform('macos', false)).toBe('darwin')
    expect(copilotPlatform('windows', false)).toBe('win32')
    expect(copilotPlatform('linux', true)).toBe('linuxmusl')
  })

  it('rejects a Cargo artifact for a different profile', () => {
    vi.stubEnv('CARGO_TARGET_DIR', '')
    vi.stubEnv('FAKE_BAD_ARTIFACT', '1')
    expect(() => packageRust(fixture())).toThrow('expected host binary')
  })

  it('supports an explicit release profile and rejects debug artifacts', () => {
    vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', 'release')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const output = packageRust(root)
    expect(
      JSON.parse(readFileSync(path.join(output, 'meta/package.json'), 'utf8'))
        .profile
    ).toBe('release')
    expect(
      spawn.mock.calls
        .filter(
          call => call[0] === 'cargo' && (call[1] as string[])[0] === 'build'
        )
        .every(call => (call[1] as string[]).includes('--release'))
    ).toBe(true)
    vi.stubEnv('FAKE_BAD_ARTIFACT', '1')
    expect(() => packageRust(root)).toThrow('expected host binary')
  })

  it('rejects unknown package profiles before running build commands', () => {
    vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', 'custom')
    expect(() => packageRust(fixture())).toThrow('must be debug or release')
    expect(spawn).not.toHaveBeenCalled()
  })

  it('rejects a mismatched installed SDK version', () => {
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const name = `copilot-sdk-${sdkPlatform}-${process.arch}`
    write(
      root,
      `app/node_modules/@github/${name}/package.json`,
      JSON.stringify({ name: `@github/${name}`, version: '0.0.1' })
    )
    expect(() => packageRust(root)).toThrow('Unexpected Copilot SDK package')
  })

  it.skipIf(process.platform === 'win32')(
    'copies a linked SDK independently of the checkout',
    () => {
      vi.stubEnv('CARGO_TARGET_DIR', '')
      const root = fixture()
      const name = `copilot-sdk-${sdkPlatform}-${process.arch}`
      const source = `sdk-source`
      write(
        root,
        `${source}/package.json`,
        JSON.stringify({ name: `@github/${name}`, version: '1.2.3' })
      )
      write(
        root,
        `${source}/prebuilds/${sdkPlatform}-${process.arch}/copilot-runtime${suffix}`,
        'runtime'
      )
      write(
        root,
        `${source}/prebuilds/${sdkPlatform}-${process.arch}/runtime.node`,
        'addon'
      )
      const link = path.join(root, 'app/node_modules/@github', name)
      mkdirSync(path.dirname(link), { recursive: true })
      symlinkSync(path.join(root, source), link, 'dir')
      const output = packageRust(root)
      rmSync(path.join(root, source), { recursive: true })
      expect(
        readFileSync(
          path.join(
            output,
            'bin/copilot-sdk/prebuilds',
            `${sdkPlatform}-${process.arch}`,
            `copilot-runtime${suffix}`
          ),
          'utf8'
        )
      ).toBe('runtime')
    }
  )

  it('preserves an existing package if a required input is absent', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const previous = `dist/Test Desktop-dev-${host}-${process.arch}/previous`
    write(root, previous, 'keep')
    rmSync(path.join(root, 'out/vite-dist/highlighter.js'))
    expect(() => packageRust(root)).toThrow('highlighter.js')
    expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
  })

  it('preserves an existing package when the renderer notice list is invalid', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const previous = `dist/Test Desktop-dev-${host}-${process.arch}/previous`
    write(root, previous, 'keep')
    write(root, 'out/vite-dist/static/licenses.json', '{}')
    expect(() => packageRust(root)).toThrow(
      'license notice list is empty or invalid'
    )
    expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
    for (const contents of ['{"bad":null}', '{"bad":{"license":""}}']) {
      write(root, 'out/vite-dist/static/licenses.json', contents)
      expect(() => packageRust(root)).toThrow(
        'license notice list is empty or invalid'
      )
      expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
    }
  })

  it('refuses packaging without the bundled emoji license', () => {
    const root = fixture()
    rmSync(path.join(root, 'gemoji/LICENSE'))
    expect(() => packageRust(root)).toThrow('gemoji')
    expect(spawn).not.toHaveBeenCalled()
  })

  it('preserves an existing package when Cargo metadata is unavailable', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    vi.stubEnv('FAKE_METADATA_ERROR', '1')
    const root = fixture()
    const previous = `dist/Test Desktop-dev-${host}-${process.arch}/previous`
    write(root, previous, 'keep')
    expect(() => packageRust(root)).toThrow(
      'Could not inspect locked Cargo metadata'
    )
    expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
  })

  it.skipIf(process.platform === 'win32')(
    'preserves an existing package when a crate license is a symlink',
    () => {
      vi.stubEnv('RELEASE_CHANNEL', 'development')
      vi.stubEnv('CARGO_TARGET_DIR', '')
      const root = fixture()
      const previous = `dist/Test Desktop-dev-${host}-${process.arch}/previous`
      write(root, previous, 'keep')
      const privateFile = path.join(root, 'private')
      writeFileSync(privateFile, 'private')
      symlinkSync(
        privateFile,
        path.join(root, 'rust/crates/desktop-app/LICENSE')
      )
      expect(() => packageRust(root)).toThrow('symbolic link')
      expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
    }
  )

  it('preserves an existing package when the release build fails', () => {
    vi.stubEnv('RELEASE_CHANNEL', 'development')
    vi.stubEnv('DESKTOP_RUST_PACKAGE_PROFILE', 'release')
    vi.stubEnv('CARGO_TARGET_DIR', '')
    const root = fixture()
    const previous = `dist/Test Desktop-dev-${host}-${process.arch}/previous`
    write(root, previous, 'keep')
    const normalSpawn = spawn.getMockImplementation()
    if (normalSpawn === undefined) {
      throw new Error('missing spawn fixture')
    }
    spawn.mockImplementation((...args: unknown[]) => {
      if (args[0] === 'cargo') {
        return { status: 1, stdout: '', stderr: '' }
      }
      return normalSpawn(...args)
    })
    expect(() => packageRust(root)).toThrow('cargo build --release')
    expect(readFileSync(path.join(root, previous), 'utf8')).toBe('keep')
  })
})
