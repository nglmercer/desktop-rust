import { Repository } from '../../models/repository'
import { isAbsolute } from 'path'
import { backend } from '../../backend/client'
import type { BackendReadableGitSetting } from '../../backend/client'

type GlobalGitSetting = 'user.name' | 'user.email' | 'init.defaultbranch'
type RepositoryGitIdentity = 'user.name' | 'user.email'

function supportedGlobalSetting(name: string): GlobalGitSetting {
  const key = name.toLowerCase()
  if (
    key === 'user.name' ||
    key === 'user.email' ||
    key === 'init.defaultbranch'
  ) {
    return key
  }
  throw new Error(`Unsupported global Git setting: ${name}`)
}

function supportedRepositorySetting(name: string): BackendReadableGitSetting {
  switch (name) {
    case 'core.autocrlf':
    case 'core.safecrlf':
    case 'pull.ff':
    case 'pull.rebase':
    case 'trailer.separators':
    case 'commit.gpgsign':
      return name
    default:
      throw new Error(`Unsupported repository Git setting: ${name}`)
  }
}

function repositoryIdentity(name: string): RepositoryGitIdentity | null {
  return name === 'user.name' || name === 'user.email' ? name : null
}

/**
 * Look up an allowlisted config value by name in the repository.
 *
 * Arbitrary configuration is intentionally not exposed through the renderer.
 * Tests and development tools that need arbitrary keys must invoke Git directly.
 */
export async function getConfigValue(
  repository: Repository,
  name: string,
  onlyLocal: boolean = false
): Promise<string | null> {
  const identity = repositoryIdentity(name)
  if (identity !== null) {
    return backend().repositoryGitIdentityValue(
      repository.path,
      identity,
      onlyLocal
    )
  }
  return backend().repositoryGitReadSetting(
    repository.path,
    supportedRepositorySetting(name),
    onlyLocal,
    false
  )
}

/** Look up an allowlisted global config value by name. */
export async function getGlobalConfigValue(
  name: string
): Promise<string | null> {
  return backend().globalGitSettingValue(supportedGlobalSetting(name))
}

/**
 * Read `commit.gpgsign` with Git boolean canonicalization.
 * This is the only boolean repository setting consumed by the renderer.
 */
export async function getBooleanConfigValue(
  repository: Repository,
  name: string,
  onlyLocal: boolean = false
): Promise<boolean | null> {
  if (name !== 'commit.gpgsign') {
    throw new Error(`Unsupported boolean Git setting: ${name}`)
  }
  const value = await backend().repositoryGitReadSetting(
    repository.path,
    name,
    onlyLocal,
    true
  )
  return value === null ? null : value === 'true'
}

/** Set one local identity value. */
export async function setConfigValue(
  repository: Repository,
  name: string,
  value: string
): Promise<void> {
  const identity = repositoryIdentity(name)
  if (identity === null) {
    throw new Error(`Unsupported repository Git setting: ${name}`)
  }
  await backend().setRepositoryGitIdentityValue(
    repository.path,
    identity,
    value
  )
}

/** Set one allowlisted global config value. */
export async function setGlobalConfigValue(
  name: string,
  value: string
): Promise<void> {
  await backend().setGlobalGitSetting(supportedGlobalSetting(name), value)
}

/**
 * Adds an existing absolute directory to `safe.directory` if it's not already
 * present, allowing Git to use that directory when it belongs to another user.
 *
 * Resolves the canonical path and rejects values that Git would interpret as
 * an exception for multiple directories.
 */
export async function addSafeDirectory(path: string) {
  if (!isAbsolute(path)) {
    throw new Error('A directory exception requires an absolute path.')
  }

  await backend().trustRepository(path)
}

/** Remove one local identity value. */
export async function removeConfigValue(
  repository: Repository,
  name: string
): Promise<void> {
  const identity = repositoryIdentity(name)
  if (identity === null) {
    throw new Error(`Unsupported repository Git setting: ${name}`)
  }
  await backend().setRepositoryGitIdentityValue(repository.path, identity, null)
}
