// `os` for the webview build. Host facts come from the Rust shell, which
// injects `window.__desktopEnv` before any page script runs.
interface IDesktopEnv {
  readonly platform?: string
  readonly arch?: string
  readonly homedir?: string
  readonly tmpdir?: string
  readonly release?: string
  readonly hostname?: string
}

const env = (): IDesktopEnv => (globalThis as any).__desktopEnv ?? {}

export const EOL = __WIN32__ ? '\r\n' : '\n'

export function platform() {
  return env().platform ?? (__WIN32__ ? 'win32' : __DARWIN__ ? 'darwin' : 'linux')
}
export function arch() {
  return env().arch ?? 'x64'
}
export function release() {
  return env().release ?? ''
}
export function homedir() {
  return env().homedir ?? ''
}
export function tmpdir() {
  return env().tmpdir ?? ''
}
export function hostname() {
  return env().hostname ?? 'localhost'
}
export function type() {
  const p = platform()
  return p === 'win32' ? 'Windows_NT' : p === 'darwin' ? 'Darwin' : 'Linux'
}
export function cpus() {
  return new Array(navigator.hardwareConcurrency || 1).fill({ model: '', speed: 0 })
}
export function totalmem() {
  return 0
}
export function freemem() {
  return 0
}
export function userInfo() {
  return { username: '', homedir: homedir(), shell: null }
}

export default {
  EOL, platform, arch, release, homedir, tmpdir, hostname, type, cpus,
  totalmem, freemem, userInfo,
}
