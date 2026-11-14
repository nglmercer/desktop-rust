import { build, type InlineConfig } from 'vite'

export type DevWatcher = Extract<
  Awaited<ReturnType<typeof build>>,
  { on: unknown }
>

/** Rebuild one bundle on changes without deleting adjacent worker bundles. */
export async function watchRustBundle(
  configFile: string | false,
  overrides: InlineConfig = {}
): Promise<{ watcher: DevWatcher; ready: Promise<void> }> {
  const result = await build({
    ...overrides,
    configFile,
    mode: 'development',
    build: { ...overrides.build, watch: {}, emptyOutDir: false },
  })
  if (Array.isArray(result) || !('on' in result)) {
    throw new Error(`Vite did not start a watcher for ${configFile}`)
  }
  const watcher = result
  const ready = new Promise<void>((resolve, reject) => {
    let initial = true
    let failed = false
    watcher.on('event', event => {
      if (event.code === 'START') {
        failed = false
      } else if (event.code === 'ERROR') {
        failed = true
        if (initial) {
          initial = false
          reject(event.error)
        } else {
          console.error(
            `Vite rebuild failed (${configFile}); fix the error to retry.`
          )
        }
      } else if (event.code === 'END' && !failed) {
        if (initial) {
          initial = false
          resolve()
        } else {
          console.log(
            `Vite rebuilt ${configFile}; after Git operations finish, use View > Reload (Ctrl/Cmd+Alt+R).`
          )
        }
      }
    })
  })
  // Other watchers may still be initializing when this build fails.
  void ready.catch(() => {})
  return { watcher, ready }
}
