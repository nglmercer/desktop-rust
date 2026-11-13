// @vitest-environment node
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { build, mergeConfig } from 'vite'
import { describe, it } from 'vitest'
import rendererConfig from '../../vite.config'

async function stopProcessGroup(pid: number, closed: Promise<void>) {
  const signal = (name: NodeJS.Signals | 0) => {
    try {
      process.kill(-pid, name)
      return true
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ESRCH') {
        return false
      }
      throw error
    }
  }
  signal('SIGTERM')
  let deadline = Date.now() + 5_000
  while (signal(0) && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  if (signal(0)) {
    signal('SIGKILL')
  }
  deadline = Date.now() + 5_000
  while (signal(0) && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  if (signal(0)) {
    throw new Error('Wry process group did not terminate; fixture retained')
  }
  await closed
}

// Requires a debug shell, Xvfb, Linux user/mount namespaces and a Konsole path.
describe.skipIf(
  process.env.DESKTOP_WRY_WEBVIEW_TEST !== '1' || process.platform !== 'linux'
)('Wry repository terminal', () => {
  it('opens an unregistered repository and confines the requested directory', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-wry-terminal-'))
    const home = path.join(root, 'home')
    const output = path.join(root, 'bundle')
    const entry = path.join(root, 'probe.ts')
    const binary = path.resolve('rust/target/debug/github-desktop-rs')
    const mountpoint = path.resolve('out/vite-dist')
    const terminal = '/usr/bin/konsole'
    const terminalFixture = path.join(root, 'terminal-fixture')
    const capture = path.join(root, 'terminal-capture')
    const repository = path.join(root, 'repository with spaces')
    const directory = path.join(repository, 'nested ; dollar $()')
    const outside = path.join(root, 'outside')
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: home }
    for (const key of Object.keys(env)) {
      if (key.startsWith('GIT_')) {
        delete env[key]
      }
    }
    delete env.WAYLAND_DISPLAY
    Object.assign(env, {
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
      GIT_TERMINAL_PROMPT: '0',
      DESKTOP_USER_DATA: path.join(home, 'user-data'),
      DESKTOP_TERMINAL_CAPTURE: capture,
      XDG_CONFIG_HOME: path.join(home, 'config'),
      XDG_DATA_HOME: path.join(home, 'data'),
      XDG_CACHE_HOME: path.join(home, 'cache'),
      XDG_RUNTIME_DIR: path.join(home, 'runtime'),
      DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(root, 'no-dbus')}`,
      GDK_BACKEND: 'x11',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      NO_AT_BRIDGE: '1',
    })
    let safeToDelete = true
    try {
      assert.ok(existsSync(binary), 'Build the debug desktop-app binary first')
      assert.ok(existsSync(mountpoint), 'Build the web renderer first')
      assert.ok(
        existsSync(terminal),
        'The fixture requires /usr/bin/konsole as a mount target'
      )
      for (const item of ['config', 'data', 'cache', 'runtime']) {
        mkdirSync(path.join(home, item), { recursive: true, mode: 0o700 })
      }
      mkdirSync(directory, { recursive: true })
      mkdirSync(outside)
      symlinkSync(outside, path.join(repository, 'escape'))
      execFileSync(
        'git',
        ['-c', 'init.templateDir=', 'init', '-q', repository],
        { env, stdio: 'pipe' }
      )
      writeFileSync(
        terminalFixture,
        '#!/bin/sh\nprintf "%s\\n" "$PWD" "$@" > "$DESKTOP_TERMINAL_CAPTURE"\n'
      )
      chmodSync(terminalFixture, 0o700)
      writeFileSync(
        entry,
        `
import { BackendClient } from ${JSON.stringify(
          path.resolve('app/src/backend/client')
        )}
;(window as any).runProbe = async () => {
  const client = new BackendClient()
  const available = await client.availableRustTerminals()
  if (!available.some(item => item.name === 'Konsole')) throw new Error('Konsole fixture not discovered')
  for (const [name, directory] of ${JSON.stringify([
    ['/bin/sh', directory],
    ['Konsole', outside],
    ['Konsole', path.join(repository, 'escape')],
  ])}) {
    let rejected = false
    try { await client.openRepositoryTerminal(${JSON.stringify(
      repository
    )}, name, directory) } catch { rejected = true }
    if (!rejected) throw new Error('unsafe terminal request was accepted')
  }
  await client.openRepositoryTerminal(${JSON.stringify(
    repository
  )}, 'Konsole', ${JSON.stringify(directory)})
  return 'opened'
}
`
      )
      await build(
        mergeConfig(rendererConfig, {
          logLevel: 'silent',
          build: {
            outDir: output,
            emptyOutDir: true,
            rollupOptions: { input: entry },
            lib: {
              entry,
              formats: ['iife'],
              name: 'WryTerminalProbe',
              fileName: () => 'probe.js',
            },
          },
        })
      )
      writeFileSync(
        path.join(output, 'index.html'),
        `<!doctype html><meta charset="UTF-8"><script src="./probe.js"></script><script>window.runProbe().then(value => console.error('WRY_TERMINAL_SUCCESS ' + JSON.stringify(value)), error => console.error('WRY_TERMINAL_FAILURE ' + JSON.stringify({message:String(error),stack:String(error && error.stack)})))</script>`
      )
      // Both bind mounts exist only in this private mount namespace. The real
      // /usr/bin/konsole and shared Vite output remain unchanged on the host.
      const child = spawn(
        'unshare',
        [
          '--user',
          '--map-root-user',
          '--mount',
          '--propagation',
          'private',
          'sh',
          '-c',
          'mount --bind "$1" "$2" && mount --bind "$3" "$4" || exit; exec xvfb-run -a "$5"',
          'sh',
          output,
          mountpoint,
          terminalFixture,
          terminal,
          binary,
        ],
        { detached: true, env, stdio: ['ignore', 'pipe', 'pipe'] }
      )
      safeToDelete = false
      const closed = new Promise<void>(resolve =>
        child.once('close', () => resolve())
      )
      try {
        const message = await new Promise<string>((resolve, reject) => {
          let text = ''
          const timeout = setTimeout(
            () => reject(new Error(`Wry probe timed out: ${text}`)),
            60_000
          )
          const collect = (chunk: Buffer) => {
            text = (text + chunk.toString()).slice(-32_768)
            const match = text.match(
              /WRY_TERMINAL_(SUCCESS|FAILURE) ([^\r\n]*)/
            )
            if (!match) {
              return
            }
            clearTimeout(timeout)
            if (match[1] === 'SUCCESS') {
              resolve(match[2])
            } else {
              reject(new Error(`Wry terminal probe failed: ${match[2]}`))
            }
          }
          child.stdout.on('data', collect)
          child.stderr.on('data', collect)
          child.once('error', error => {
            clearTimeout(timeout)
            reject(error)
          })
          child.once('exit', code => {
            clearTimeout(timeout)
            reject(new Error(`Wry exited ${code}: ${text}`))
          })
        })
        assert.equal(JSON.parse(message), 'opened')
        assert.equal(
          readFileSync(capture, 'utf8'),
          `${directory}\n--workdir\n${directory}\n`
        )
        const registry = path.join(home, 'user-data', 'repositories.json')
        if (existsSync(registry)) {
          assert.ok(
            !readFileSync(registry, 'utf8').includes(repository),
            'the operation must not silently register the repository'
          )
        }
      } finally {
        if (child.pid !== undefined) {
          await stopProcessGroup(child.pid, closed)
        } else {
          await closed
        }
        safeToDelete = true
      }
    } finally {
      if (safeToDelete) {
        rmSync(root, { recursive: true, force: true })
      }
    }
  }, 120_000)
})
