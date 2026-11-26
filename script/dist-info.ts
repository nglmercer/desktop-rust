import { directoryOf } from './esm-utils'
import * as Path from 'path'
import { getProductName, getVersion } from '../app/package-info'

const productName = getProductName()
const version = getVersion()

const projectRoot = Path.join(directoryOf(import.meta.url), '..')

export function getDistRoot() {
  return Path.join(projectRoot, 'dist')
}

export function getDistPath() {
  return Path.join(
    getDistRoot(),
    `${getExecutableName()}-${process.platform}-${getDistArchitecture()}`
  )
}

export function getExecutableName() {
  const suffix = process.env.NODE_ENV === 'development' ? '-dev' : ''

  if (process.platform === 'win32') {
    return `${getWindowsIdentifierName()}${suffix}`
  } else if (process.platform === 'linux') {
    return 'desktop'
  } else {
    return productName
  }
}

export function getWindowsIdentifierName() {
  return 'GitHubDesktop'
}

export const isPublishable = () =>
  ['production', 'beta', 'test'].includes(getChannel())

export const getChannel = () =>
  process.env.RELEASE_CHANNEL ?? process.env.NODE_ENV ?? 'development'

export function getDistArchitecture(): 'arm64' | 'x64' {
  // If a specific npm_config_arch is set, we use that one instead of the OS arch (to support cross compilation)
  if (
    process.env.npm_config_arch === 'arm64' ||
    process.env.npm_config_arch === 'x64'
  ) {
    return process.env.npm_config_arch
  }

  if (process.arch === 'arm64') {
    return 'arm64'
  }

  // TODO: Check if it's x64 running on an arm64 Windows with IsWow64Process2
  // More info: https://www.rudyhuyn.com/blog/2017/12/13/how-to-detect-that-your-x86-application-runs-on-windows-on-arm/
  // Right now (March 3, 2021) is not very important because support for x64
  // apps on an arm64 Windows is experimental. See:
  // https://blogs.windows.com/windows-insider/2020/12/10/introducing-x64-emulation-in-preview-for-windows-10-on-arm-pcs-to-the-windows-insider-program/

  return 'x64'
}

export function getUpdatesURL() {
  // It is also possible to use a `x64/` path, but for now we'll leave the
  // original URL without architecture in it (which will still work for
  // compatibility reasons) in case anything goes wrong until we have everything
  // sorted out.
  const architecturePath = getDistArchitecture() === 'arm64' ? 'arm64/' : ''
  return `https://central.github.com/api/deployments/desktop/desktop/${architecturePath}latest?version=${version}&env=${getChannel()}`
}
