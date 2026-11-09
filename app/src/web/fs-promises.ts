// `fs/promises` for the webview build; see ./fs.ts.
import { promises, constants, FileHandle } from './fs'

export { constants, FileHandle }
export const {
  open, readFile, writeFile, appendFile, stat, lstat, access, readdir, mkdir,
  mkdtemp, rm, rmdir, unlink, rename, copyFile, cp, realpath, readlink,
  symlink, chmod, truncate, opendir,
} = promises

export default promises
