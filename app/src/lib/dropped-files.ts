import { webUtils } from 'electron'

/** Translate DOM files for the legacy Electron shell. Web renderers cannot
 * reveal native paths and return no paths here. */
export function pathsForDroppedFiles(files: FileList): ReadonlyArray<string> {
  return Array.from(files, webUtils.getPathForFile).filter(
    path => path.length > 0
  )
}
