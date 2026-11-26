import { homedir } from '../web/os'

const home = homedir()

export function untildify(path: string): string {
  return home ? path.replace(/^~(?=$|\/|\\)/, home) : path
}
