import { getDefaultBranch } from '../helpers/default-branch'
import { backend } from '../../backend/client'

/** Init a new git repository in the given path. */
export async function initGitRepository(path: string): Promise<void> {
  const branch = await getDefaultBranch()
  await backend().initializeDesktopRepository(path, branch)
}
