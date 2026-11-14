import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  chmodSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import * as path from 'node:path'
import { collectRustLicenseInventory } from './rust-license-inventory'
import { createIsolatedViteOutput } from './vite-output'
import { acquireRustPackageLock } from './rust-package-lock'

type Platform = 'linux' | 'macos' | 'windows'
type Architecture = 'x64' | 'arm64'

interface IAppPackage {
  readonly productName: string
  readonly version: string
  readonly bundleID: string
  readonly dependencies: Record<string, string>
}

function hostPlatform(platform: NodeJS.Platform): Platform {
  switch (platform) {
    case 'linux':
      return 'linux'
    case 'darwin':
      return 'macos'
    case 'win32':
      return 'windows'
    default:
      throw new Error(`Unsupported host platform: ${platform}`)
  }
}

function hostArchitecture(arch: string): Architecture {
  if (arch === 'x64' || arch === 'arm64') return arch
  throw new Error(`Unsupported host architecture: ${arch}`)
}

function distributionDirectory(repo: string): string {
  const configured = process.env.DESKTOP_RUST_DIST_DIR
  if (configured && !path.isAbsolute(configured)) {
    throw new Error('DESKTOP_RUST_DIST_DIR must be an absolute path')
  }
  return configured ? path.resolve(configured) : path.join(repo, 'dist')
}

function productName(app: IAppPackage, channel: string): string {
  const product =
    channel === 'development' ? `${app.productName}-dev` : app.productName
  if (
    !product ||
    /[\\/\0]/.test(product) ||
    product === '.' ||
    product === '..'
  ) {
    throw new Error('Invalid product name')
  }
  return product
}

function appPackage(repo: string): IAppPackage {
  return JSON.parse(
    readFileSync(path.join(repo, 'app/package.json'), 'utf8')
  ) as IAppPackage
}

/** Protect one package destination while it is staged or executed. */
export function acquirePackageOutputLock(
  repo: string,
  channel: string
): () => void {
  const platform = hostPlatform(process.platform)
  const arch = hostArchitecture(process.arch)
  const product = productName(appPackage(repo), channel)
  const dist = distributionDirectory(repo)
  mkdirSync(dist, { recursive: true })
  const canonical = realpathSync(dist)
  // Case-folding only serializes extra distinct destinations on case-sensitive
  // filesystems; it also protects aliases on case-insensitive filesystems.
  const key = `${product}-${platform}-${arch}`.toLowerCase()
  const digest = createHash('sha256').update(key).digest('hex')
  return acquireRustPackageLock(
    path.join(canonical, `.desktop-package-${digest}.lock`)
  )
}

/** SDK platform names follow Node's OS naming, unlike package labels. */
export function copilotPlatform(
  platform: Platform,
  linuxMusl: boolean
): string {
  switch (platform) {
    case 'linux':
      return linuxMusl ? 'linuxmusl' : 'linux'
    case 'macos':
      return 'darwin'
    case 'windows':
      return 'win32'
  }
}

function requiredFile(file: string): void {
  if (!existsSync(file) || !statSync(file).isFile()) {
    throw new Error(`Required file missing: ${file}`)
  }
}

function copyOptional(source: string, destination: string): void {
  if (existsSync(source)) cpSync(source, destination, { recursive: true })
}

/** Only replace a directory bearing the metadata of a previous Rust package. */
function assertManagedOutput(
  output: string,
  product: string,
  bundleID: string,
  platform: Platform,
  arch: Architecture
): void {
  let entry
  try {
    entry = lstatSync(output)
  } catch (error) {
    if (
      error &&
      typeof error === 'object' &&
      'code' in error &&
      error.code === 'ENOENT'
    )
      return
    throw error
  }
  const refuse = () => {
    throw new Error(
      `Refusing to replace an unrecognized package directory: ${output}`
    )
  }
  if (!entry.isDirectory() || entry.isSymbolicLink()) refuse()
  const meta = path.join(output, 'meta')
  const manifest = path.join(meta, 'package.json')
  try {
    const directory = lstatSync(meta)
    const file = lstatSync(manifest)
    if (
      !directory.isDirectory() ||
      directory.isSymbolicLink() ||
      !file.isFile() ||
      file.isSymbolicLink()
    )
      refuse()
    const value: unknown = JSON.parse(readFileSync(manifest, 'utf8'))
    if (
      value === null ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      !('productName' in value) ||
      value.productName !== product ||
      !('bundleID' in value) ||
      value.bundleID !== bundleID ||
      !('platform' in value) ||
      value.platform !== platform ||
      !('arch' in value) ||
      value.arch !== arch ||
      !('executable' in value) ||
      value.executable !==
        `bin/desktop-rs${platform === 'windows' ? '.exe' : ''}`
    )
      refuse()
  } catch {
    refuse()
  }
}

/** Resolve the executable from a previously staged Rust package. */
export function stagedRustDesktop(
  repo: string,
  expected: { readonly channel: string; readonly profile: 'debug' | 'release' }
): string {
  const platform = hostPlatform(process.platform)
  const arch = hostArchitecture(process.arch)
  const app = appPackage(repo)
  const product = productName(app, expected.channel)
  const output = path.join(
    distributionDirectory(repo),
    `${product}-${platform}-${arch}`
  )
  assertManagedOutput(output, product, app.bundleID, platform, arch)
  const bin = path.join(output, 'bin')
  const executable = path.join(
    bin,
    `github-desktop-rs${platform === 'windows' ? '.exe' : ''}`
  )
  try {
    const metadata: unknown = JSON.parse(
      readFileSync(path.join(output, 'meta', 'package.json'), 'utf8')
    )
    if (
      metadata === null ||
      typeof metadata !== 'object' ||
      Array.isArray(metadata) ||
      !('channel' in metadata) ||
      metadata.channel !== expected.channel ||
      !('profile' in metadata) ||
      metadata.profile !== expected.profile ||
      !('version' in metadata) ||
      metadata.version !== app.version
    ) {
      throw new Error('package profile, channel or version does not match')
    }
    const directory = lstatSync(bin)
    const file = lstatSync(executable)
    if (
      !directory.isDirectory() ||
      directory.isSymbolicLink() ||
      !file.isFile() ||
      file.isSymbolicLink()
    ) {
      throw new Error('invalid executable')
    }
  } catch {
    throw new Error(
      `Rust package is unavailable at ${output}; run pnpm package first`
    )
  }
  return executable
}

/** Package only the host architecture: Cargo is not being cross-compiled here. */
export function packageRust(
  repo: string,
  requestedPlatform: string = 'auto'
): string {
  if (process.env.DESKTOP_VITE_DIST !== undefined) {
    throw new Error('package:rust manages its own Vite output directory')
  }
  const channel = process.env.RELEASE_CHANNEL || 'development'
  const production =
    channel === 'production' ||
    process.env.DESKTOP_RUST_PACKAGE_PROFILE === 'release'
  if (
    production &&
    process.env.NODE_ENV !== undefined &&
    process.env.NODE_ENV !== 'production'
  ) {
    throw new Error('Production packages require NODE_ENV=production')
  }
  const buildEnvironment = {
    ...process.env,
    RELEASE_CHANNEL: channel,
    ...(production ? { NODE_ENV: 'production' } : {}),
  }
  const releaseDestination = acquirePackageOutputLock(repo, channel)
  let releaseTarget: (() => void) | undefined
  try {
    const base = process.env.CARGO_TARGET_DIR
      ? path.resolve(repo, process.env.CARGO_TARGET_DIR)
      : path.join(repo, 'rust/target')
    mkdirSync(base, { recursive: true })
    const target = path.join(realpathSync(base), 'desktop-packaged')
    mkdirSync(target, { recursive: true })
    const targetStat = lstatSync(target)
    if (!targetStat.isDirectory() || targetStat.isSymbolicLink())
      throw new Error('Package target must be a real directory')
    releaseTarget = acquireRustPackageLock(
      path.join(target, '.desktop-package-target.lock')
    )
    const output = createIsolatedViteOutput()
    try {
      return packageRustWithOutput(
        repo,
        requestedPlatform,
        output.directory,
        target,
        buildEnvironment
      )
    } finally {
      output.dispose()
    }
  } finally {
    try {
      releaseTarget?.()
    } finally {
      releaseDestination()
    }
  }
}

function packageRustWithOutput(
  repo: string,
  requestedPlatform: string,
  ui: string,
  target: string,
  buildEnvironment: NodeJS.ProcessEnv
): string {
  const platform = hostPlatform(process.platform)
  const arch = hostArchitecture(process.arch)
  const profile = process.env.DESKTOP_RUST_PACKAGE_PROFILE ?? 'debug'
  const dist = distributionDirectory(repo)
  if (profile !== 'debug' && profile !== 'release') {
    throw new Error('DESKTOP_RUST_PACKAGE_PROFILE must be debug or release')
  }
  if (process.env.CARGO_BUILD_TARGET) {
    throw new Error(
      'Explicit Cargo targets are not supported by the host package'
    )
  }
  if (requestedPlatform !== 'auto' && requestedPlatform !== platform) {
    throw new Error(
      `Cannot package ${requestedPlatform} on ${platform}: build on the target platform`
    )
  }
  for (const file of [
    'LICENSE',
    'rust/THIRD_PARTY_NOTICES.txt',
    'gemoji/LICENSE',
  ]) {
    requiredFile(path.join(repo, file))
  }
  const app = appPackage(repo)
  const rustVersion = readFileSync(
    path.join(repo, 'rust/crates/core/src/copilot.rs'),
    'utf8'
  ).match(/pub const COPILOT_SDK_VERSION: &str = "([^"]+)";/)?.[1]
  const sdkVersion = app.dependencies['@github/copilot-sdk']
  if (!rustVersion || !sdkVersion || rustVersion !== sdkVersion) {
    throw new Error('The Rust and application Copilot SDK versions must match')
  }
  const rustc = spawnSync('rustc', ['-vV'], { cwd: repo, encoding: 'utf8' })
  if (rustc.error) throw rustc.error
  if (rustc.status !== 0)
    throw new Error('Could not determine the Rust host target')
  const triple = rustc.stdout.match(/^host: (.+)$/m)?.[1]
  const archPrefix = arch === 'x64' ? 'x86_64-' : 'aarch64-'
  const platformPart = platform === 'macos' ? 'darwin' : platform
  if (!triple?.startsWith(archPrefix) || !triple.includes(platformPart)) {
    throw new Error('Rust host target does not match the packaging host')
  }
  const channel = process.env.RELEASE_CHANNEL || 'development'
  const product = productName(app, channel)
  const vite = path.join(repo, 'node_modules/vite/bin/vite.js')
  requiredFile(vite)
  for (const args of [
    ['build'],
    ['build', '-c', 'vite.highlighter.config.ts'],
    ['build', '-c', 'vite.ipc-shim.config.ts'],
  ]) {
    const result = spawnSync(process.execPath, [vite, ...args], {
      cwd: repo,
      stdio: 'inherit',
      env: { ...buildEnvironment, DESKTOP_VITE_DIST: ui },
    })
    if (result.error) throw result.error
    if (result.status !== 0) throw new Error(`Vite ${args.join(' ')} failed`)
  }
  const suffix = platform === 'windows' ? '.exe' : ''
  const runCargo = (args: string[], name: string): string => {
    const result = spawnSync(
      'cargo',
      [...args, '--message-format=json-render-diagnostics'],
      {
        cwd: repo,
        encoding: 'utf8',
        maxBuffer: 64 * 1024 * 1024,
        env: {
          ...buildEnvironment,
          DESKTOP_VITE_DIST: ui,
          CARGO_TARGET_DIR: target,
          DESKTOP_REQUIRE_NATIVE_PLATFORM: '1',
        },
      }
    )
    if (result.error) throw result.error
    if (result.stderr) process.stderr.write(result.stderr)
    if (result.status !== 0) throw new Error(`cargo ${args.join(' ')} failed`)
    const artifacts = result.stdout
      .split('\n')
      .filter(Boolean)
      .map(
        line =>
          JSON.parse(line) as {
            reason: string
            target?: { name?: string }
            executable?: string | null
          }
      )
    const artifact = artifacts.find(
      message =>
        message.reason === 'compiler-artifact' &&
        message.target?.name === name &&
        message.executable
    )?.executable
    const expected = path.join(target, profile, `${name}${suffix}`)
    if (!artifact || path.resolve(artifact) !== path.resolve(expected)) {
      throw new Error(
        `Cargo did not produce the expected host binary ${expected}`
      )
    }
    return artifact
  }
  // The optional embedded UI is necessary for the CLI's browser frontend.
  // The packaged-ui feature prevents the shell from using the build
  // checkout's out/vite-dist after relocation, even with debug overrides.
  const cli = runCargo(
    [
      'build',
      ...(profile === 'release' ? ['--release'] : []),
      '--locked',
      '-p',
      'desktop-cli',
      '--features',
      'embedded-ui,packaged-runtime',
    ],
    'desktop-rs'
  )
  const gui = runCargo(
    [
      'build',
      ...(profile === 'release' ? ['--release'] : []),
      '--locked',
      '-p',
      'desktop-app',
      '--features',
      'packaged-ui',
    ],
    'github-desktop-rs'
  )
  for (const file of [
    cli,
    gui,
    path.join(ui, 'index.html'),
    path.join(ui, 'highlighter.js'),
    path.join(ui, 'ipc-shim.js'),
    path.join(ui, 'desktop-auth-scheme'),
    path.join(ui, 'static/licenses.json'),
    path.join(ui, 'static/LICENSE.choosealicense.md'),
    path.join(repo, 'gemoji/db/emoji.json'),
  ])
    requiredFile(file)
  let webNotices: unknown
  try {
    webNotices = JSON.parse(
      readFileSync(path.join(ui, 'static/licenses.json'), 'utf8')
    )
  } catch {
    throw new Error('The renderer license notice list is invalid')
  }
  if (
    webNotices === null ||
    typeof webNotices !== 'object' ||
    Array.isArray(webNotices) ||
    Object.keys(webNotices).length === 0 ||
    !Object.values(webNotices).every(
      notice =>
        notice !== null &&
        typeof notice === 'object' &&
        !Array.isArray(notice) &&
        'license' in notice &&
        typeof notice.license === 'string' &&
        notice.license.trim().length > 0
    )
  ) {
    throw new Error('The renderer license notice list is empty or invalid')
  }

  const linuxMusl = platform === 'linux' && triple.includes('musl')
  const sdkPlatform = copilotPlatform(platform, linuxMusl)
  const sdkName = `copilot-sdk-${sdkPlatform}-${arch}`
  let sdk = path.join(repo, 'app/node_modules/@github', sdkName)
  if (!existsSync(sdk)) {
    const store = path.join(repo, 'node_modules/.pnpm')
    sdk = path.join(
      store,
      `@github+${sdkName}@${sdkVersion}`,
      'node_modules/@github',
      sdkName
    )
  }
  if (existsSync(sdk)) {
    const packageInfo = JSON.parse(
      readFileSync(path.join(sdk, 'package.json'), 'utf8')
    ) as { name?: string; version?: string }
    if (
      packageInfo.name !== `@github/${sdkName}` ||
      packageInfo.version !== sdkVersion
    ) {
      throw new Error(`Unexpected Copilot SDK package at ${sdk}`)
    }
    const prebuilds = path.join(sdk, 'prebuilds', `${sdkPlatform}-${arch}`)
    requiredFile(path.join(prebuilds, `copilot-runtime${suffix}`))
    requiredFile(path.join(prebuilds, 'runtime.node'))
  } else {
    console.warn(
      `Copilot SDK package ${sdkName} was not found; Copilot is unavailable`
    )
  }

  mkdirSync(dist, { recursive: true })
  const output = path.join(dist, `${product}-${platform}-${arch}`)
  const staging = mkdtempSync(path.join(dist, '.package-rust-'))
  const backup = `${staging}-previous`
  let hadPrevious = false
  try {
    for (const dir of ['bin', 'ui', 'assets', 'licenses', 'meta'])
      mkdirSync(path.join(staging, dir))
    collectRustLicenseInventory(repo, triple, staging)
    cpSync(cli, path.join(staging, 'bin', `desktop-rs${suffix}`))
    cpSync(gui, path.join(staging, 'bin', `github-desktop-rs${suffix}`))
    if (platform !== 'windows') {
      chmodSync(path.join(staging, 'bin', `desktop-rs${suffix}`), 0o755)
      chmodSync(path.join(staging, 'bin', `github-desktop-rs${suffix}`), 0o755)
    }
    cpSync(ui, path.join(staging, 'ui'), { recursive: true })
    copyOptional(path.join(ui, 'assets'), path.join(staging, 'assets/assets'))
    // Rust discovers the bundled SDK beside the executable, not at the
    // package root. Keep the complete package, including its metadata.
    if (existsSync(sdk))
      cpSync(sdk, path.join(staging, 'bin/copilot-sdk'), {
        recursive: true,
        dereference: true,
      })
    for (const [source, name] of [
      [path.join(repo, 'LICENSE'), 'LICENSE'],
      [
        path.join(repo, 'rust/THIRD_PARTY_NOTICES.txt'),
        'THIRD_PARTY_NOTICES.txt',
      ],
      [path.join(repo, 'gemoji/LICENSE'), 'gemoji-LICENSE'],
      [path.join(ui, 'static/licenses.json'), 'web-notices.json'],
      [
        path.join(ui, 'static/LICENSE.choosealicense.md'),
        'choosealicense-LICENSE',
      ],
    ]) {
      cpSync(source, path.join(staging, 'licenses', name), {
        dereference: true,
      })
    }
    copyOptional(
      path.join(repo, 'app/LICENSE'),
      path.join(staging, 'licenses/app-LICENSE')
    )
    writeFileSync(
      path.join(staging, 'meta/package.json'),
      `${JSON.stringify(
        {
          productName: product,
          bundleID: app.bundleID,
          version: app.version,
          channel,
          platform,
          arch,
          profile,
          executable: `bin/desktop-rs${suffix}`,
        },
        null,
        2
      )}\n`
    )
    writeFileSync(
      path.join(staging, 'meta/runtime.json'),
      `${JSON.stringify(
        {
          note: 'Git and Git LFS are external tools managed by Rust; install them using the platform package manager.',
          git: 'required',
          lfs: 'optional but recommended for LFS repositories',
        },
        null,
        2
      )}\n`
    )
    assertManagedOutput(output, product, app.bundleID, platform, arch)
    if (existsSync(output)) {
      renameSync(output, backup)
      hadPrevious = true
    }
    try {
      renameSync(staging, output)
    } catch (error) {
      if (hadPrevious) renameSync(backup, output)
      throw error
    }
    if (hadPrevious) rmSync(backup, { recursive: true, force: true })
  } finally {
    rmSync(staging, { recursive: true, force: true })
  }
  return output
}

if (require.main === module) {
  const args = process.argv.slice(2)
  if (args.includes('--help')) {
    console.log(
      'Usage: node --import tsx script/package-rust.ts [auto|linux|macos|windows]'
    )
    console.log(
      'Builds and stages the Rust desktop and embedded browser UI for this host.'
    )
    console.log(
      'Set DESKTOP_RUST_PACKAGE_PROFILE=release for an optimized package.'
    )
    console.log('Set DESKTOP_RUST_DIST_DIR to an absolute output directory.')
  } else if (args.length > 1) {
    console.error('Expected at most one platform argument')
    process.exitCode = 1
  } else {
    try {
      const output = packageRust(path.resolve(__dirname, '..'), args[0])
      console.log(`Package ready: ${output}`)
    } catch (error) {
      console.error(
        `package-rust: ${
          error instanceof Error ? error.message : String(error)
        }`
      )
      process.exitCode = 1
    }
  }
}
