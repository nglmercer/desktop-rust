import { shell } from '../../lib/app-shell'
import { Dispatcher } from '../dispatcher'

export async function openFile(
  fullPath: string,
  dispatcher: Dispatcher
): Promise<void> {
  const errorMessage = await shell.openPath(fullPath)

  if (errorMessage !== '') {
    const error = {
      name: 'no-external-program',
      message: `Unable to open file ${fullPath} in an external program. ${errorMessage}`,
    }
    await dispatcher.postError(error)
  }
}
