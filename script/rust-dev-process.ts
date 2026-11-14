import { spawn, type ChildProcess } from 'node:child_process'

function signalGroup(pid: number, signal: NodeJS.Signals | 0): boolean {
  try {
    process.kill(-pid, signal)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') {
      return false
    }
    throw error
  }
}

async function waitForGroup(
  pid: number,
  milliseconds: number
): Promise<boolean> {
  const deadline = Date.now() + milliseconds
  while (signalGroup(pid, 0)) {
    if (Date.now() >= deadline) {
      return false
    }
    await new Promise(resolve => setTimeout(resolve, 50))
  }
  return true
}

/** Cancel Cargo and its build-script descendants, escalating if they ignore signals. */
export async function stopDevelopmentBuild(child: ChildProcess): Promise<void> {
  const pid = child.pid
  if (pid === undefined) {
    return
  }
  if (process.platform === 'win32') {
    const killer = spawn('taskkill.exe', ['/PID', String(pid), '/T', '/F'], {
      stdio: 'ignore',
      windowsHide: true,
    })
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        killer.kill()
        reject(new Error('Timed out stopping the Cargo process tree'))
      }, 5_000)
      killer.once('error', error => {
        clearTimeout(timer)
        reject(error)
      })
      killer.once('close', code => {
        clearTimeout(timer)
        if (code === 0) {
          resolve()
        } else {
          reject(
            new Error('Could not confirm that the Cargo process tree stopped')
          )
        }
      })
    })
    return
  }
  for (const [signal, delay] of [
    ['SIGINT', 750],
    ['SIGTERM', 750],
    ['SIGKILL', 2_000],
  ] as const) {
    if (!signalGroup(pid, signal) || (await waitForGroup(pid, delay))) {
      return
    }
  }
  throw new Error('Could not confirm that the Cargo process group stopped')
}
