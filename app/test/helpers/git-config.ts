import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { createTempDirectory, type TestCleanupContext } from './temp'

/**
 * Isolate a test from inherited global and system Git configuration.
 *
 * Register with beforeEach in suites that need predictable remotes and URL
 * rewrites. Tests may override these variables; teardown restores their original
 * values. Since the environment is process-wide, these tests must run serially.
 */
export async function isolateGitConfig(
  t: TestCleanupContext
): Promise<void> {
  const config = join(await createTempDirectory(t), 'empty-config')
  await writeFile(config, '')

  const environment = {
    GIT_CONFIG_GLOBAL: config,
    GIT_CONFIG_SYSTEM: config,
    GIT_CONFIG_NOSYSTEM: '1',
  }
  const previous = Object.keys(environment).map(name => ({
    name,
    value: process.env[name],
  }))

  const restore = () => {
    for (const { name, value } of previous) {
      if (value === undefined) {
        delete process.env[name]
      } else {
        process.env[name] = value
      }
    }
  }
  if ('after' in t) {
    t.after(restore)
  } else {
    t.onTestFinished(restore)
  }

  for (const [name, value] of Object.entries(environment)) {
    process.env[name] = value
  }
}
