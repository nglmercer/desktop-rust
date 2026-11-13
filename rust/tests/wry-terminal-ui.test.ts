// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
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

// The probe creates a repository through the actual shared React UI, then
// activates the native Repository menu in the Wry window. Both bind mounts are
// private to the spawned Linux user/mount namespace.
describe.skipIf(
  process.env.DESKTOP_WRY_WEBVIEW_TEST !== '1' || process.platform !== 'linux'
)('Wry terminal UI', () => {
  it('creates a repository and opens it in the selected terminal', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-wry-terminal-ui-'))
    const home = path.join(root, 'home')
    const output = path.join(root, 'bundle')
    const terminalFixture = path.join(root, 'terminal-fixture')
    const capture = path.join(root, 'terminal-capture')
    const ready = path.join(root, 'ready')
    const terminal = '/usr/bin/konsole'
    const binary = path.resolve('rust/target/debug/github-desktop-rs')
    const mountpoint = path.resolve('out/vite-dist')
    const repository = path.join(home, 'terminal ui repository')
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
      writeFileSync(
        path.join(root, 'gitconfig'),
        '[user]\n\tname = Terminal Test\n\temail = test@example.invalid\n'
      )
      writeFileSync(
        terminalFixture,
        '#!/bin/sh\nprintf "%s\\n" "$PWD" "$@" > "$DESKTOP_TERMINAL_CAPTURE"\n'
      )
      chmodSync(terminalFixture, 0o700)

      await build(
        mergeConfig(rendererConfig, {
          logLevel: 'silent',
          build: { outDir: output, emptyOutDir: true },
        })
      )
      const index = path.join(output, 'index.html')
      const probe = `
<script>
localStorage.setItem('shell', 'Konsole')
;(async () => {
  const delay = () => new Promise(resolve => setTimeout(resolve, 100))
  const until = async (description, find) => {
    const end = Date.now() + 45000
    while (Date.now() < end) {
      const value = find()
      if (value) return value
      await delay()
    }
    throw new Error('Timed out waiting for ' + description + ': ' + document.body.innerText.slice(0, 4000))
  }
  const button = text => [...document.querySelectorAll('button, a')].find(item => item.textContent.trim().includes(text) && !item.disabled)
  const setInput = (input, value) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, value)
    input.dispatchEvent(new Event('input', { bubbles: true }))
    input.dispatchEvent(new Event('change', { bubbles: true }))
  }
  ;(await until('skip sign in', () => button('Skip this step'))).click()
  ;(await until('finish git setup', () => button('Finish'))).click()
  ;(await until('create repository action', () => button('Create a New Repository on your local drive'))).click()
  setInput(await until('repository name', () => document.querySelector('input[placeholder="repository name"]')), 'terminal ui repository')
  setInput(await until('repository path', () => document.querySelector('input[placeholder="repository path"]')), ${JSON.stringify(
    home
  )})
  ;(await until('create repository button', () => button('Create repository'))).click()
  await until('selected repository', () => document.body.innerText.includes('terminal ui repository') && document.body.innerText.includes('Current repository'))
  console.error('WRY_TERMINAL_UI_READY')
})().catch(error => console.error('WRY_TERMINAL_UI_FAILURE ' + JSON.stringify({message:String(error),stack:String(error && error.stack)})))
</script>`
      writeFileSync(
        index,
        readFileSync(index, 'utf8').replace('</body>', `${probe}</body>`)
      )

      const command =
        'mount --bind "$1" "$2" && mount --bind "$3" "$4" || exit; exec xvfb-run -a sh -c \'"$1" & app=$!; n=0; while [ ! -f "$2" ] && [ "$n" -lt 900 ]; do sleep .1; n=$((n+1)); done; [ -f "$2" ] || { kill "$app"; wait "$app"; exit 1; }; window=$(xdotool search --onlyvisible --name "GitHub Desktop \\(Rust\\)" | head -n 1); [ -n "$window" ] || { kill "$app"; wait "$app"; exit 1; }; xdotool windowfocus --sync "$window"; sleep 1; xdotool key --clearmodifiers alt+r; sleep .3; xdotool key --clearmodifiers p; wait "$app"\' sh "$5" "$6"'
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
          command,
          'sh',
          output,
          mountpoint,
          terminalFixture,
          terminal,
          binary,
          ready,
        ],
        { detached: true, env, stdio: ['ignore', 'pipe', 'pipe'] }
      )
      safeToDelete = false
      const closed = new Promise<void>(resolve =>
        child.once('close', () => resolve())
      )
      try {
        await new Promise<void>((resolve, reject) => {
          let text = ''
          const timer = setInterval(() => {
            if (!existsSync(capture)) {
              return
            }
            clearTimeout(timeout)
            clearInterval(timer)
            resolve()
          }, 100)
          const timeout = setTimeout(() => {
            clearInterval(timer)
            reject(new Error(`UI probe timed out: ${text}`))
          }, 90_000)
          const collect = (chunk: Buffer) => {
            text = (text + chunk.toString()).slice(-32_768)
            if (text.includes('WRY_TERMINAL_UI_FAILURE')) {
              clearTimeout(timeout)
              clearInterval(timer)
              reject(new Error(text))
            }
            if (text.includes('WRY_TERMINAL_UI_READY') && !existsSync(ready)) {
              writeFileSync(ready, '')
            }
          }
          child.stdout.on('data', collect)
          child.stderr.on('data', collect)
          child.once('error', error => {
            clearTimeout(timeout)
            clearInterval(timer)
            reject(error)
          })
          child.once('exit', code => {
            clearTimeout(timeout)
            clearInterval(timer)
            reject(new Error(`Wry exited ${code}: ${text}`))
          })
        })
        assert.equal(
          readFileSync(capture, 'utf8'),
          `${repository}\n--workdir\n${repository}\n`
        )
        assert.ok(existsSync(path.join(repository, '.git')))
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
  }, 180_000)
})
