import { spawn, type ChildProcess } from 'node:child_process'
import * as path from 'node:path'
import { stopDevelopmentBuild } from './rust-dev-process'
import { installDevelopmentStopSignals } from './rust-dev-signals'
import { createIsolatedViteOutput } from './vite-output'

/** Build a production renderer and both Rust hosts without touching the dev bundle. */
export async function buildRustProduction(
  repo: string,
  inherited: NodeJS.ProcessEnv = process.env
): Promise<void> {
  if (inherited.DESKTOP_VITE_DIST !== undefined) {
    throw new Error('build:prod manages its own Vite output directory')
  }

  const output = createIsolatedViteOutput()
  const bundle = output.directory
  const env: NodeJS.ProcessEnv = {
    ...inherited,
    NODE_ENV: 'production',
    RELEASE_CHANNEL: 'production',
    DESKTOP_REQUIRE_NATIVE_PLATFORM: '1',
    DESKTOP_VITE_DIST: bundle,
  }
  let active: ChildProcess | undefined
  let interrupted = false
  let stopping: Promise<void> | undefined
  const removeSignals = installDevelopmentStopSignals(() => {
    interrupted = true
    if (active) stopping = stopDevelopmentBuild(active)
  })
  let clean = true
  const run = async (program: string, args: ReadonlyArray<string>) => {
    if (interrupted) throw new Error('Production build interrupted')
    const child = spawn(program, [...args], {
      cwd: repo,
      env,
      detached: true,
      windowsHide: true,
      stdio: 'inherit',
    })
    active = child
    const outcome = await new Promise<{
      code: number | null
      signal: NodeJS.Signals | null
    }>((resolve, reject) => {
      child.once('error', reject)
      child.once('close', (code, signal) => resolve({ code, signal }))
    }).finally(() => {
      active = undefined
    })
    if (interrupted || outcome.code !== 0 || outcome.signal !== null) {
      if (!stopping) stopping = stopDevelopmentBuild(child)
      throw new Error(
        interrupted
          ? 'Production build interrupted'
          : `${program} failed (${outcome.signal ?? outcome.code})`
      )
    }
  }
  try {
    const vite = path.join(repo, 'node_modules/vite/bin/vite.js')
    for (const args of [
      ['build'],
      ['build', '-c', 'vite.highlighter.config.ts'],
      ['build', '-c', 'vite.ipc-shim.config.ts'],
    ]) {
      await run(process.execPath, [vite, ...args])
    }
    await run('cargo', [
      'build',
      '--release',
      '--locked',
      '-p',
      'desktop-app',
      '-p',
      'desktop-cli',
      '--features',
      'desktop-app/packaged-ui,desktop-cli/embedded-ui,desktop-cli/packaged-runtime',
    ])
  } finally {
    if (stopping) {
      try {
        await stopping
      } catch (error) {
        clean = false
        console.error(
          `Could not stop the production build; retained ${path.dirname(
            bundle
          )}`,
          error
        )
      }
    }
    removeSignals()
    if (clean) {
      output.dispose()
    }
  }
}

if (require.main === module) {
  void buildRustProduction(path.resolve(__dirname, '..')).catch(error => {
    console.error(error)
    process.exitCode = 1
  })
}
