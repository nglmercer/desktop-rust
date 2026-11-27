import { which } from './executable-path'

/** Attempts to locate the path to the system version of Git. */
export const findGitOnPath = async () => {
  try {
    return (await which('git', { nothrow: true })) ?? undefined
  } catch (error) {
    log.warn(`Failed trying to find Git on PATH`, error)
    return undefined
  }
}
