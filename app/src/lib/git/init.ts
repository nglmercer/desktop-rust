import { getDefaultBranch } from '../helpers/default-branch'
import { git } from './core'
import { backend } from '../../backend/client'

/** Init a new git repository in the given path. */
export async function initGitRepository(path: string): Promise<void> {
  const branch = await getDefaultBranch()
  if (typeof window !== 'undefined' && backend().transportKind !== 'none') {
    await backend().initializeDesktopRepository(path, branch)
    return
  }
  await git(
    ['-c', `init.defaultBranch=${branch}`, 'init'],
    path,
    'initGitRepository'
  )
}
