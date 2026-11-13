import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import * as path from 'node:path'

type Platform = 'linux' | 'macos' | 'windows'
type Architecture = 'x64' | 'arm64'

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

/** Package only the host architecture: Cargo is not being cross-compiled here. */
export function packageRust(
  repo: string,
  requestedPlatform: string = 'auto'
): string {
  const platform = hostPlatform(process.platform)
  const arch = hostArchitecture(process.arch)
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
  const app = JSON.parse(
    readFileSync(path.join(repo, 'app/package.json'), 'utf8')
  ) as {
    productName: string
    version: string
    bundleID: string
    dependencies: Record<string, string>
  }
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
  const vite = path.join(repo, 'node_modules/vite/bin/vite.js')
  requiredFile(vite)
  for (const args of [
    ['build'],
    ['build', '-c', 'vite.highlighter.config.ts'],
  ]) {
    const result = spawnSync(process.execPath, [vite, ...args], {
      cwd: repo,
      stdio: 'inherit',
    })
    if (result.error) throw result.error
    if (result.status !== 0) throw new Error(`Vite ${args.join(' ')} failed`)
  }
  const suffix = platform === 'windows' ? '.exe' : ''
  const target = process.env.CARGO_TARGET_DIR
    ? path.resolve(repo, process.env.CARGO_TARGET_DIR)
    : path.join(repo, 'rust/target')
  const runCargo = (args: string[], name: string): string => {
    const result = spawnSync(
      'cargo',
      [...args, '--message-format=json-render-diagnostics'],
      {
        cwd: repo,
        encoding: 'utf8',
        maxBuffer: 64 * 1024 * 1024,
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
    const expected = path.join(target, 'debug', `${name}${suffix}`)
    if (!artifact || path.resolve(artifact) !== path.resolve(expected)) {
      throw new Error(
        `Cargo did not produce the expected host binary ${expected}`
      )
    }
    return artifact
  }
  // The optional embedded UI is necessary for the CLI's browser frontend.
  const cli = runCargo(
    ['build', '--locked', '-p', 'desktop-cli', '--features', 'embedded-ui'],
    'desktop-rs'
  )
  const gui = runCargo(
    ['build', '--locked', '-p', 'desktop-app'],
    'github-desktop-rs'
  )
  const ui = path.join(repo, 'out/vite-dist')
  for (const file of [
    cli,
    gui,
    path.join(ui, 'index.html'),
    path.join(ui, 'highlighter.js'),
    path.join(repo, 'gemoji/db/emoji.json'),
  ])
    requiredFile(file)

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

  const dist = path.join(repo, 'dist')
  mkdirSync(dist, { recursive: true })
  const output = path.join(dist, `${product}-${platform}-${arch}`)
  const staging = mkdtempSync(path.join(dist, '.package-rust-'))
  const backup = `${staging}-previous`
  let hadPrevious = false
  try {
    for (const dir of ['bin', 'ui', 'assets', 'licenses', 'meta'])
      mkdirSync(path.join(staging, dir))
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
    for (const file of [
      'LICENSE',
      'rust/THIRD_PARTY_NOTICES.txt',
      'app/LICENSE',
    ]) {
      copyOptional(
        path.join(repo, file),
        path.join(
          staging,
          'licenses',
          file === 'app/LICENSE' ? 'app-LICENSE' : path.basename(file)
        )
      )
    }
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
