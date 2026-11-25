/** Hex digest of a file, computed by the Rust shell without copying data here. */
export const getFileHash = (
  path: string,
  type: 'sha1' | 'sha256'
): Promise<string> =>
  (globalThis as any).__desktop.invoke('desktop.io.fs-hash', {
    path,
    algorithm: type,
  })
