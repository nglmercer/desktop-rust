import * as Path from 'path'
import { hostApp } from '../../web/host-bridge'

let logDirectoryPath: string | null = null

export function getLogDirectoryPath() {
  if (!logDirectoryPath) {
    const userData = hostApp.getPath('userData')
    logDirectoryPath = Path.join(userData, 'logs')
  }

  return logDirectoryPath
}
