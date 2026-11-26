import * as OS from '../web/os'
import memoizeOne from 'memoize-one'

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

export function compareSystemVersions(
  left: string,
  right: string
): -1 | 0 | 1 | undefined {
  const parse = (version: string) => {
    if (!/^\d+(?:\.\d+)*$/.test(version)) return undefined
    return version.split('.').map(part => parseInt(part, 10))
  }
  const leftParts = parse(left)
  const rightParts = parse(right)
  if (leftParts === undefined || rightParts === undefined) return undefined
  const length = Math.max(leftParts.length, rightParts.length)
  for (let index = 0; index < length; index++) {
    const leftPart = leftParts[index] ?? 0
    const rightPart = rightParts[index] ?? 0
    if (leftPart < rightPart) return -1
    if (leftPart > rightPart) return 1
  }
  return 0
}

function getSystemVersionSafe() {
  return __DARWIN__ ? macOSVersionFromDarwinRelease(OS.release()) : OS.release()
}

function systemVersionGreaterThanOrEqualTo(version: string) {
  const sysver = getSystemVersionSafe()
  if (sysver === undefined) return false
  const comparison = compareSystemVersions(sysver, version)
  return comparison !== undefined && comparison >= 0
}

function systemVersionLessThan(version: string) {
  const sysver = getSystemVersionSafe()
  if (sysver === undefined) return false
  const comparison = compareSystemVersions(sysver, version)
  return comparison !== undefined && comparison < 0
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
