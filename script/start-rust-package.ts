import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import * as path from 'node:path'
import { acquirePackageOutputLock, stagedRustDesktop } from './package-rust'
import { installDevelopmentStopSignals } from './rust-dev-signals'

/** Start the staged desktop and request its normal shutdown when signalled. */
async function main(): Promise<void> {
  const repo = path.resolve(__dirname, '..')
  const releaseLock = acquirePackageOutputLock(repo, 'production')
  let safeToRelease = true
  try {
    await runDesktop(repo, () => { safeToRelease = false }, () => { safeToRelease = true })
  } finally {
    if (safeToRelease) {
      releaseLock()
    } else {
      console.error('Package lock retained: inspect the desktop process and its work before removing it.')
    }
  }
}

async function runDesktop(repo: string, markRunning: () => void, markDrained: () => void): Promise<void> {
  const executable = stagedRustDesktop(repo, {
    channel: 'production',
    profile: 'release',
  })
  const control = mkdtempSync(path.join(tmpdir(), 'desktop-rust-package-'))
  const quitFile = path.join(control, 'quit')
  const resultFile = path.join(control, 'result')
  let host: ChildProcess | undefined
  let interrupted = false
  let failed = false
  let forceTimer: ReturnType<typeof setTimeout> | undefined
  let hardTimer: ReturnType<typeof setTimeout> | undefined
  let rejectHardTimeout: ((error: Error) => void) | undefined
  let didExit = false
  const hardTimeout = new Promise<never>((_, reject) => {
    rejectHardTimeout = reject
  })

  const stop = () => {
    if (interrupted) return
    interrupted = true
    if (!host) return
    try {
      writeFileSync(quitFile, '')
    } catch (error) {
      failed = true
      console.error('Could not request a graceful desktop shutdown:', error)
      host.kill('SIGKILL')
    }
    // The host allows five seconds for its own drain. If the GUI cannot
    // process the request, terminate it and report the uncertain outcome.
    forceTimer = setTimeout(() => {
      failed = true
      console.error('The desktop did not drain; forcing termination.')
      host?.kill('SIGKILL')
      hardTimer = setTimeout(() => {
        rejectHardTimeout?.(new Error('The desktop did not exit; inspect the process before retrying.'))
      }, 2_000)
    }, 8_000)
  }
  const removeSignals = installDevelopmentStopSignals(stop)
  try {
    host = spawn(executable, process.argv.slice(2), {
      cwd: repo,
      env: { ...process.env, DESKTOP_PACKAGE_QUIT_FILE: quitFile },
      stdio: 'inherit',
      detached: true,
      windowsHide: true,
    })
    if (host.pid !== undefined) markRunning()
    const running = host
    const result = await Promise.race([
      new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
        (resolve, reject) => {
          running.once('error', reject)
          running.once('close', (code, signal) => resolve({ code, signal }))
        }
      ),
      hardTimeout,
    ])
    didExit = true
    let drained = false
    try {
      drained = readFileSync(resultFile, 'utf8') === 'drained'
    } catch {
      // A missing acknowledgement means shutdown could not be verified.
    }
    if (failed || result.code !== 0 || result.signal !== null || !drained) {
      console.error(`Desktop host exited (${result.signal ?? result.code})`)
      if (!drained) console.error('Desktop shutdown was not confirmed as drained.')
      process.exitCode = 1
    } else {
      markDrained()
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  } finally {
    if (forceTimer) clearTimeout(forceTimer)
    if (hardTimer) clearTimeout(hardTimer)
    removeSignals()
    if (didExit || host?.pid === undefined) {
      rmSync(control, { recursive: true, force: true })
    } else {
      // Preserve the request when the OS did not confirm host termination.
      host.unref()
      console.error(`Shutdown request retained at ${quitFile}`)
    }
  }
}

void main().catch(error => {
  console.error(error instanceof Error ? error.message : String(error))
  process.exitCode = 1
})
