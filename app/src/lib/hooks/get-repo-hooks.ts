import { backend } from '../../backend/client'

const hookDiscoveryEnvironmentNames = new Set([
  'LOCAL_GIT_DIRECTORY',
  'GIT_CONFIG',
  'GIT_DIR',
  'GIT_COMMON_DIR',
  'GIT_WORK_TREE',
  'GIT_OBJECT_DIRECTORY',
  'GIT_ALTERNATE_OBJECT_DIRECTORIES',
  'GIT_INDEX_FILE',
  'GIT_SHALLOW_FILE',
  'GIT_GRAFT_FILE',
  'GIT_REPLACE_REF_BASE',
  'GIT_NO_REPLACE_OBJECTS',
  'GIT_NAMESPACE',
  'GIT_CONFIG_SYSTEM',
  'GIT_CONFIG_GLOBAL',
  'GIT_CONFIG_NOSYSTEM',
  'GIT_CONFIG_PARAMETERS',
  'GIT_CONFIG_COUNT',
  'HOME',
  'XDG_CONFIG_HOME',
])

function hookDiscoveryEnvironment(
  environment: Record<string, string | undefined> | undefined
): Record<string, string | undefined> | undefined {
  if (environment === undefined) {
    return undefined
  }
  return Object.fromEntries(
    Object.entries(environment).filter(
      ([key]) =>
        hookDiscoveryEnvironmentNames.has(key) ||
        key.startsWith('GIT_CONFIG_KEY_') ||
        key.startsWith('GIT_CONFIG_VALUE_')
    )
  )
}

/** Resolve the same traditional hook directory used for discovery. */
export const getRepoHooksPath = async (
  path: string,
  env?: Record<string, string | undefined>
) =>
  (
    await backend().repositoryRendererHooks(
      path,
      undefined,
      hookDiscoveryEnvironment(env)
    )
  ).path

/**
 * Returns the names of executable Git hooks found in the given repository.
 *
 * @param path   The file system path to the Git repository (root of working
 *               directory).
 * @param filter An optional array of hook names to filter the results.
 *               Including '*' will return all hooks.
 * @param env    The same Git environment as the operation being intercepted.
 */
export async function* getRepoHooks(
  path: string,
  filter?: string[],
  env?: Record<string, string | undefined>
) {
  const { hooks } = await backend().repositoryRendererHooks(
    path,
    filter,
    hookDiscoveryEnvironment(env)
  )
  yield* hooks
}
