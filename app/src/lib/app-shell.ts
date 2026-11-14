import * as Path from 'path'

import { Repository } from '../models/repository'
import {
  showItemInFolder,
  showFolderContents,
  openExternal,
  moveItemToTrash,
} from '../ui/main-process-proxy'
import { openPath } from './open-path'
import { backend } from '../backend/client'

export interface IAppShell {
  readonly moveItemToTrash: (path: string) => Promise<void>
  readonly openExternal: (path: string) => Promise<boolean>
  /**
   * Reveals the specified file using the operating
   * system default application.
   * Do not use this method with non-validated paths.
   *
   * @param path - The path of the file to open
   */

  readonly openPath: (path: string) => Promise<string>
  /**
   * Reveals the specified file on the operating system
   * default file explorer. If a folder is passed, it will
   * open its parent folder and preselect the passed folder.
   *
   * @param path - The path of the file to show
   */
  readonly showItemInFolder: (path: string) => void
  /**
   * Reveals the specified folder on the operating
   * system default file explorer.
   * Do not use this method with non-validated paths.
   *
   * @param path - The path of the folder to open
   */
  readonly showFolderContents: (path: string) => void
}

export const shell: IAppShell = {
  // Since Electron 13, shell.trashItem doesn't work from the renderer process
  // on Windows. Therefore, we must invoke it from the main process. See
  // https://github.com/electron/electron/issues/29598
  moveItemToTrash,
  openExternal,
  showItemInFolder,
  showFolderContents,
  openPath,
}

/** Open a saved repository in the host file manager from either renderer. */
export async function showRepositoryInFileManager(
  path: string,
  onError: (error: Error) => void | Promise<void>
): Promise<void> {
  try {
    if (typeof window !== 'undefined' && backend().transportKind === 'ws') {
      await backend().revealRegisteredRepository(path)
      return
    }
    await showFolderContents(path)
  } catch (error) {
    try {
      await onError(error instanceof Error ? error : new Error(String(error)))
    } catch (reportingError) {
      log.error(
        'Failed to report the file manager error',
        reportingError instanceof Error ? reportingError : new Error(String(reportingError))
      )
    }
  }
}

/**
 * Reveals a file from a repository in the native file manager.
 *
 * @param repository The currently active repository instance
 * @param path The path of the file relative to the root of the repository
 */
export function revealInFileManager(repository: Repository, path: string) {
  const fullyQualifiedFilePath = Path.join(repository.path, path)
  return shell.showItemInFolder(fullyQualifiedFilePath)
}
