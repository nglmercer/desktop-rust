import {
  execFile as execFileOrig,
  type ExecFileOptions,
} from 'child_process'

/**
 * A version of execFile which returns a Promise rather than the traditional
 * callback approach of `child_process.execFile`.
 *
 * The promise resolves with `{ stdout, stderr }` and rejects with the same
 * fields attached to the error, as Node's promisified execFile does. This is
 * written out rather than using `util.promisify`: the webview's `util`
 * polyfill resolves with stdout alone.
 *
 * See `child_process.execFile` for more information
 */
export function execFile(
  file: string,
  args?: ReadonlyArray<string>,
  options?: ExecFileOptions & { readonly encoding?: BufferEncoding }
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    execFileOrig(
      file,
      args ? [...args] : [],
      options ?? {},
      (error, stdout, stderr) => {
        if (error) {
          reject(Object.assign(error, { stdout, stderr }))
        } else {
          resolve({ stdout: String(stdout), stderr: String(stderr) })
        }
      }
    )
  })
}
