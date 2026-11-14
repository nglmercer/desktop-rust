import * as OS from 'os'
import compareVersions from 'compare-versions'
import memoizeOne from 'memoize-one'

const { compare } = compareVersions

/**
 * The macOS product version for a Darwin kernel release (for example
 * `23.4.0` is macOS 14.4). Darwin 20 started macOS 11 and Darwin 25 macOS 26;
 * earlier releases are macOS 10.x.
 */
export function macOSVersionFromDarwinRelease(
  release: string
): string | undefined {
  const [major, minor = '0', patch = '0'] = release.split('.')
  const darwin = parseInt(major, 10)
  if (Number.isNaN(darwin)) {
    return undefined
  }
  if (darwin >= 25) {
    return `${darwin + 1}.${minor}.${patch}`
  }
  if (darwin >= 20) {
    return `${darwin - 9}.${minor}.${patch}`
  }
  return darwin >= 4 ? `10.${darwin - 4}.${minor}` : undefined
}

function getSystemVersionSafe() {
  return __DARWIN__ ? macOSVersionFromDarwinRelease(OS.release()) : OS.release()
}

function systemVersionGreaterThanOrEqualTo(version: string) {
  const sysver = getSystemVersionSafe()
  return sysver === undefined ? false : compare(sysver, version, '>=')
}

function systemVersionLessThan(version: string) {
  const sysver = getSystemVersionSafe()
  return sysver === undefined ? false : compare(sysver, version, '<')
}

/** Get the OS we're currently running on. */
export function getOS(): string {
  const version = getSystemVersionSafe()
  if (__DARWIN__) {
    return `Mac OS ${version}`
  } else if (__WIN32__) {
    return `Windows ${version}`
  } else {
    return `${OS.type()} ${version}`
  }
}

/** We're currently running macOS and it is macOS Ventura. */
export const isMacOSVentura = memoizeOne(
  () =>
    __DARWIN__ &&
    systemVersionGreaterThanOrEqualTo('13.0') &&
    systemVersionLessThan('14.0')
)

/** We're currently running macOS and it is macOS Sonoma. */
export const isMacOSSonoma = memoizeOne(
  () =>
    __DARWIN__ &&
    systemVersionGreaterThanOrEqualTo('14.0') &&
    systemVersionLessThan('15.0')
)

/** We're currently running macOS and it is macOS Sequoia. */
export const isMacOSSequoia = memoizeOne(
  () =>
    __DARWIN__ &&
    systemVersionGreaterThanOrEqualTo('15.0') &&
    systemVersionLessThan('16.0')
)

/** We're currently running macOS and it is macOS Sonoma or later. */
export const isMacOSSonomaOrLater = memoizeOne(
  () => __DARWIN__ && systemVersionGreaterThanOrEqualTo('14.0')
)

/** We're currently running macOS and it is macOS Catalina or earlier. */
export const isMacOSCatalinaOrEarlier = memoizeOne(
  () => __DARWIN__ && systemVersionLessThan('10.16')
)

/** We're currently running macOS and it is at least Mojave. */
export const isMacOSMojaveOrLater = memoizeOne(
  () => __DARWIN__ && systemVersionGreaterThanOrEqualTo('10.13.0')
)

/** We're currently running macOS and it is at least Big Sur. */
export const isMacOSBigSurOrLater = memoizeOne(
  // We're using 10.16 rather than 11.0 here due to
  // Chromium-based shells report macOS 11 as 10.16, so compare against that.
  () => __DARWIN__ && systemVersionGreaterThanOrEqualTo('10.16')
)

/** We're currently running macOS and it is at least Tahoe. */
export const isMacOSTahoeOrLater = memoizeOne(
  () => __DARWIN__ && systemVersionGreaterThanOrEqualTo('26')
)

/** We're currently running Windows 10 and it is at least 1809 Preview Build 17666. */
export const isWindows10And1809Preview17666OrLater = memoizeOne(
  () => __WIN32__ && systemVersionGreaterThanOrEqualTo('10.0.17666')
)
