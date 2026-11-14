import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { watchRustBundle, type DevWatcher } from './rust-dev-watcher'
import { installDevelopmentStopSignals } from './rust-dev-signals'
import { stopDevelopmentBuild } from './rust-dev-process'

const root = path.resolve(__dirname, '..')

/** Build the native host and use Cargo's artifact path rather than guessing its target directory. */
async function buildHost(
  onStart: (child: ChildProcess) => void
): Promise<string> {
  const child = spawn(
    'cargo',
    [
      'build',
      '--locked',
      '--profile',
      'dev',
      '-p',
      'desktop-app',
      '--message-format=json-render-diagnostics',
    ],
    {
      cwd: root,
      stdio: ['ignore', 'pipe', 'inherit'],
      detached: true,
      windowsHide: true,
    }
  )
  onStart(child)
  let executable: string | undefined
  if (child.stdout === null) {
    child.kill()
    throw new Error('Cargo output is unavailable')
  }
  const lines = createInterface({ input: child.stdout })
  lines.on('line', line => {
    try {
      const message: unknown = JSON.parse(line)
      if (typeof message !== 'object' || message === null) {
        return
      }
      const event = message as {
        reason?: string
        target?: { name?: string }
        executable?: unknown
        message?: { rendered?: string }
      }
      if (event.reason === 'compiler-message' && event.message?.rendered) {
        process.stderr.write(event.message.rendered)
      }
      if (
        event.reason === 'compiler-artifact' &&
        event.target?.name === 'github-desktop-rs' &&
        typeof event.executable === 'string'
      ) {
        executable = event.executable
      }
    } catch {
      console.error(`Unexpected Cargo output: ${line}`)
    }
  })
  await new Promise<void>((resolve, reject) => {
    child.once('error', reject)
    child.once('close', (code, signal) =>
      code === 0
        ? resolve()
        : reject(new Error(`Cargo build failed (${signal ?? code})`))
    )
  })
  if (!executable || !path.isAbsolute(executable)) {
    throw new Error('Cargo did not report the desktop host executable')
  }
  const probe = spawnSync(executable, ['--check-dev-host'], {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
  })
  if (
    probe.error ||
    probe.status !== 0 ||
    probe.stdout.trim() !== 'desktop-dev-host-ok'
  ) {
    throw new Error(
      'The built host must enable debug assertions and disable packaged-ui'
    )
  }
  return executable
}

async function main(): Promise<void> {
  const control = mkdtempSync(path.join(tmpdir(), 'desktop-rust-dev-'))
  const quitFile = path.join(control, 'quit')
  const watchers: DevWatcher[] = []
  let cargo: ChildProcess | undefined
  let host: ChildProcess | undefined
  let interrupted = false
  let wakeStop: () => void = () => {}
  const stopping = new Promise<void>(resolve => {
    wakeStop = resolve
  })
  let killTimer: ReturnType<typeof setTimeout> | undefined
  let cancelBuild: Promise<void> | undefined
  const stop = () => {
    if (interrupted) {
      return
    }
    interrupted = true
    wakeStop()
    if (host) {
      try {
        writeFileSync(quitFile, '')
      } catch (error) {
        console.error('Could not request a graceful desktop shutdown:', error)
        host.kill()
      }
      // The host has a five-second drain deadline. Keep a bounded fallback if
      // the window cannot process its shutdown request.
      killTimer = setTimeout(() => host?.kill(), 8_000)
      killTimer.unref()
    } else {
      if (cargo) {
        cancelBuild = stopDevelopmentBuild(cargo)
      }
    }
  }
  const removeSignals = installDevelopmentStopSignals(stop)
  try {
    const executable = await Promise.race([
      buildHost(child => {
        cargo = child
      }),
      stopping.then(() => {
        throw new Error('Development startup interrupted')
      }),
    ])
    cargo = undefined
    if (interrupted) {
      return
    }
    const results = await Promise.allSettled([
      watchRustBundle(path.join(root, 'vite.config.ts')),
      watchRustBundle(path.join(root, 'vite.highlighter.config.ts')),
      watchRustBundle(path.join(root, 'vite.ipc-shim.config.ts')),
    ])
    const bundles: Array<{ watcher: DevWatcher; ready: Promise<void> }> = []
    for (const result of results) {
      if (result.status === 'fulfilled') {
        watchers.push(result.value.watcher)
        bundles.push(result.value)
      }
    }
    const failed = results.find(result => result.status === 'rejected')
    if (failed?.status === 'rejected') {
      throw failed.reason
    }
    await Promise.race([
      Promise.all(bundles.map(bundle => bundle.ready)),
      stopping.then(() => {
        throw new Error('Development startup interrupted')
      }),
    ])
    if (interrupted) {
      return
    }
    console.log(
      'Starting the Rust desktop. Vite rebuilds on changes; after Git operations finish, use View > Reload (Ctrl/Cmd+Alt+R).'
    )
    const launched = spawn(executable, process.argv.slice(2), {
      cwd: root,
      env: { ...process.env, DESKTOP_DEV_QUIT_FILE: quitFile },
      stdio: 'inherit',
      // Put the host in a separate process group so Ctrl-C reaches the
      // supervisor first; it then requests the host's normal shutdown.
      detached: true,
      windowsHide: true,
    })
    host = launched
    const status = await new Promise<{
      code: number | null
      signal: NodeJS.Signals | null
    }>((resolve, reject) => {
      launched.once('error', reject)
      launched.once('close', (code, signal) => resolve({ code, signal }))
    })
    host = undefined
    if (status.code !== 0 || status.signal !== null) {
      console.error(`Desktop host exited (${status.signal ?? status.code})`)
      process.exitCode = 1
    }
  } catch (error) {
    if (!interrupted) {
      console.error(error)
      process.exitCode = 1
    }
  } finally {
    if (killTimer) {
      clearTimeout(killTimer)
    }
    if (cancelBuild) {
      try {
        await cancelBuild
      } catch (error) {
        console.error(error)
        process.exitCode = 1
      }
    }
    await Promise.all(
      watchers.map(watcher =>
        watcher.close().catch(error => {
          console.error('Could not close Vite watcher:', error)
          process.exitCode = 1
        })
      )
    )
    removeSignals()
    rmSync(control, { recursive: true, force: true })
  }
}

void main()
