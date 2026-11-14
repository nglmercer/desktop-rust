import { execFileSync } from 'node:child_process'
import * as fs from 'node:fs'
import * as path from 'node:path'

const sourceDirectory = path.resolve(__dirname, '../vendor/printenvz')

/** Return the path to the installed environment printer. */
export function getPrintenvzPath(directory = sourceDirectory): string {
  return path.join(
    directory,
    'build',
    'Release',
    process.platform === 'win32' ? 'printenvz.exe' : 'printenvz'
  )
}
export function resolveRustTarget(host: string, platform: string, architecture: string): string {
  if (!['x64', 'arm64', 'ia32', 'arm'].includes(architecture)) {
    throw new Error(
      `Unsupported printenvz cross target: ${host} to ${architecture}`
    )
  }
  const native = host.split('-', 1)[0]
  const nativeArchitectures: Record<string, string> = {
    x86_64: 'x64',
    aarch64: 'arm64',
    i686: 'ia32',
    i586: 'ia32',
    arm: 'arm',
    armv7: 'arm',
  }
  const nativeArchitecture = nativeArchitectures[native]
  if (architecture === nativeArchitecture) return host

  if (platform === 'darwin' && host.endsWith('-apple-darwin')) {
    const target: Record<string, string> = {
      x64: 'x86_64-apple-darwin',
      arm64: 'aarch64-apple-darwin',
    }
    if (target[architecture]) return target[architecture]
  }
  if (platform === 'win32' && host.endsWith('-pc-windows-msvc')) {
    const target: Record<string, string> = {
      x64: 'x86_64-pc-windows-msvc',
      arm64: 'aarch64-pc-windows-msvc',
      ia32: 'i686-pc-windows-msvc',
    }
    if (target[architecture]) return target[architecture]
  }
  if (platform === 'linux' && ['x86_64', 'aarch64'].includes(native)) {
    const suffix = host.endsWith('-unknown-linux-gnu')
      ? '-unknown-linux-gnu'
      : host.endsWith('-unknown-linux-musl')
      ? '-unknown-linux-musl'
      : undefined
    const targets: Record<string, string> = { x64: 'x86_64', arm64: 'aarch64' }
    const target = targets[architecture]
    if (suffix && target) return target + suffix
  }
  if (platform === 'android') {
    const target: Record<string, string> = {
      x64: 'x86_64-linux-android',
      arm64: 'aarch64-linux-android',
      ia32: 'i686-linux-android',
      arm: 'armv7-linux-androideabi',
    }
    if (target[architecture]) return target[architecture]
  }
  throw new Error(
    `Unsupported printenvz cross target: ${host} to ${architecture}`
  )
}

export function buildPrintenvz(directory = sourceDirectory): void {
  const rustc = process.env.RUSTC || 'rustc'
  const cargo = process.env.CARGO || 'cargo'
  const version = execFileSync(rustc, ['-vV'], { encoding: 'utf8' })
  const host = version.match(/^host: (\S+)$/m)?.[1]
  if (!host || !host.includes('-')) {
    throw new Error('Unable to determine the Rust host target')
  }
  const hostOs = host.includes('-apple-darwin')
    ? 'darwin'
    : host.includes('-windows-')
    ? 'win32'
    : host.endsWith('-android') || host.endsWith('-androideabi')
    ? 'android'
    : host.includes('-linux-')
    ? 'linux'
    : undefined
  if (hostOs !== process.platform) {
    throw new Error(`Unsupported Rust host target: ${host}`)
  }

  // npm can install for another CPU (for example, an arm64 macOS package
  // built on x64). Never silently ship the build host's executable instead.
  const os = process.env.npm_config_os
  if (os && os !== process.platform) {
    throw new Error(
      `Unsupported printenvz target OS: ${os}; expected ${process.platform}`
    )
  }
  const cpu = process.env.npm_config_cpu
  const arch = process.env.npm_config_arch
  const targetArch = process.env.TARGET_ARCH
  const choices = [cpu, arch, targetArch].filter(Boolean)
  if (new Set(choices).size > 1) {
    throw new Error(`Conflicting printenvz CPU settings: ${choices.join(', ')}`)
  }
  const architecture = cpu || arch || targetArch || process.arch
  const target = resolveRustTarget(host, process.platform, architecture)
  const targetDir = path.join(directory, 'build', 'cargo')
  execFileSync(
    cargo,
    [
      'build',
      '--manifest-path',
      path.join(directory, 'Cargo.toml'),
      '--locked',
      '--release',
      '--target',
      target,
      '--target-dir',
      targetDir,
    ],
    { cwd: directory, stdio: 'inherit' }
  )
  const executable = path.join(
    targetDir,
    target,
    'release',
    process.platform === 'win32' ? 'printenvz.exe' : 'printenvz'
  )
  const destination = getPrintenvzPath(directory)
  fs.mkdirSync(path.dirname(destination), { recursive: true })
  const temporary = `${destination}.${process.pid}.tmp`
  try {
    fs.copyFileSync(executable, temporary)
    fs.renameSync(temporary, destination)
  } finally {
    fs.rmSync(temporary, { force: true })
  }
}
