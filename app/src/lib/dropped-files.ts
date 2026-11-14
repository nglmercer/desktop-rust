import { hostFiles } from '../web/host-bridge'

/** Translate DOM files for the desktop host. Web renderers cannot
 * reveal native paths and return no paths here. */
export function pathsForDroppedFiles(files: FileList): ReadonlyArray<string> {
  return Array.from(files, hostFiles.getPathForFile).filter(
    path => path.length > 0
  )
}
