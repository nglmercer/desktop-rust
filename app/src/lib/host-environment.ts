/** Scoped host facts injected by the Rust shell before renderer code runs. */
interface IDesktopHostEnvironment {
  readonly platform?: string
  readonly arch?: string
  readonly homedir?: string
  readonly tmpdir?: string
  readonly release?: string
}

const environment = (): IDesktopHostEnvironment =>
  (globalThis as any).__desktopEnv ?? {}

export function hostPlatform(): string {
  return (
    environment().platform ??
    (__WIN32__ ? 'win32' : __DARWIN__ ? 'darwin' : 'linux')
  )
}

export function hostArchitecture(): string {
  return environment().arch ?? 'x64'
}

export function hostHomeDirectory(): string {
  return environment().homedir ?? ''
}

export function hostTemporaryDirectory(): string {
  return environment().tmpdir ?? ''
}

export function hostOperatingSystemRelease(): string {
  return environment().release ?? ''
}

export function hostOperatingSystemType(): string {
  const platform = hostPlatform()
  return platform === 'win32'
    ? 'Windows_NT'
    : platform === 'darwin'
      ? 'Darwin'
      : 'Linux'
}

/** Stable Unix signal numbers needed when decoding host child-process exits. */
export const hostProcessSignals: Readonly<Record<string, number>> =
  Object.freeze({
    SIGHUP: 1,
    SIGINT: 2,
    SIGQUIT: 3,
    SIGABRT: 6,
    SIGKILL: 9,
    SIGSEGV: 11,
    SIGPIPE: 13,
    SIGTERM: 15,
  })
