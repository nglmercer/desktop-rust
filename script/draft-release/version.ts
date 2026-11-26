import { directoryOf } from '../esm-utils'
import { readFileSync, writeFileSync } from 'fs'
import { join } from 'path'
import { inc, parse, SemVer } from 'semver'

import { Channel } from './channel'

function isBetaTag(version: SemVer) {
  return version.prerelease.some(p => p.startsWith('beta'))
}

function isTestTag(version: SemVer) {
  return version.prerelease.some(p => p.startsWith('test'))
}

function tryGetBetaNumber(version: SemVer): number | null {
  if (isBetaTag(version)) {
    const tag = version.prerelease[0]
    const text = tag.substring(4)
    const betaNumber = parseInt(text, 10)
    return isNaN(betaNumber) ? null : betaNumber
  }

  return null
}

export function getNextVersionNumber(
  version: string,
  channel: Channel
): string {
  const semanticVersion = parse(version)

  if (semanticVersion == null) {
    throw new Error(`Unable to parse input '${version}' into version`)
  }

  switch (channel) {
    case 'production':
      if (isBetaTag(semanticVersion)) {
        throw new Error(
          `Unable to draft production release using beta version '${version}'`
        )
      }

      if (isTestTag(semanticVersion)) {
        throw new Error(
          `Unable to draft production release using test version '${version}'`
        )
      }

      const nextVersion = inc(version, 'patch')
      if (nextVersion == null) {
        throw new Error(
          `Unable to increment next production version from release version '${version}'`
        )
      }

      return nextVersion

    case 'beta':
      if (isTestTag(semanticVersion)) {
        throw new Error(
          `Unable to draft beta release using test version '${version}'`
        )
      }

      const betaNumber = tryGetBetaNumber(semanticVersion)

      if (betaNumber) {
        return semanticVersion.version.replace(
          `-beta${betaNumber}`,
          `-beta${betaNumber + 1}`
        )
      } else {
        const nextVersion = inc(semanticVersion, 'patch')
        const firstBeta = `${nextVersion}-beta1`
        return firstBeta
      }
    case 'test':
      if (isBetaTag(semanticVersion)) {
        throw new Error(
          `Unable to draft test release using beta version '${version}'`
        )
      }

      if (isTestTag(semanticVersion)) {
        const tag = semanticVersion.prerelease[0]
        const text = tag.substring(4)
        const testNumber = parseInt(text, 10)
        return semanticVersion.version.replace(
          `-test${testNumber}`,
          `-test${testNumber + 1}`
        )
      } else {
        const nextVersion = inc(semanticVersion, 'patch')
        const firstTest = `${nextVersion}-test1`
        return firstTest
      }
    default:
      throw new Error(
        `Resolving the next version is not implemented for channel ${channel}`
      )
  }
}

/** Set the application version without invoking npm or updating an npm lockfile. */
export function setAppPackageVersion(nextVersion: string): void {
  const packagePath = join(
    directoryOf(import.meta.url),
    '..',
    '..',
    'app',
    'package.json'
  )
  const packageInfo = JSON.parse(readFileSync(packagePath, 'utf8')) as {
    version?: string
  }
  packageInfo.version = nextVersion
  writeFileSync(packagePath, `${JSON.stringify(packageInfo, null, 2)}\n`)
}
