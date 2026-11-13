import { writeFile } from 'fs/promises'
import { join } from 'path'
import { createTempDirectory, TestCleanupContext } from './temp'

/** Isolate Git configuration and enable ownership checks for a test. */
export async function setupOwnershipCheck(t: TestCleanupContext) {
  const home = await createTempDirectory(t)
  const globalConfigPath = join(home, '.gitconfig')
  await writeFile(globalConfigPath, '[safe]\n\tdirectory =\n')

  const env = {
    HOME: home,
    XDG_CONFIG_HOME: home,
    GIT_CONFIG_GLOBAL: globalConfigPath,
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_TEST_ASSUME_DIFFERENT_OWNER: '1',
  }

  for (const [name, value] of Object.entries(env)) {
    const previous = process.env[name]
    const restore = () => {
      if (previous === undefined) {
        delete process.env[name]
      } else {
        process.env[name] = previous
      }
    }
    if ('after' in t) {
      t.after(restore)
    } else {
      t.onTestFinished(restore)
    }
    process.env[name] = value
  }

  return globalConfigPath
}
