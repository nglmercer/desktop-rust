import { hostHomeDirectory } from './host-environment'

export function untildify(path: string): string {
  const home = hostHomeDirectory()
  return home ? path.replace(/^~(?=$|\/|\\)/, home) : path
}
