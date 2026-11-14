// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { build, mergeConfig } from 'vite'
import { describe, it } from 'vitest'
import rendererConfig from '../../vite.config'

// Builds its own Cargo target and Vite bundle. This is deliberately opt-in:
// the first run may need several minutes and gigabytes of temporary disk.
const enabled = process.env.DESKTOP_TEST_WRY_MARKDOWN === '1'

function signalGroup(child: ChildProcess, signal: NodeJS.Signals | 0): boolean {
  if (child.pid === undefined) return false
  try {
    process.kill(-child.pid, signal)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false
    throw error
  }
}

/** Stop all processes before allowing their private files to be deleted. */
async function stopGroup(child: ChildProcess): Promise<void> {
  if (!signalGroup(child, 0)) return
  signalGroup(child, 'SIGTERM')
  const wait = async (milliseconds: number) => {
    const deadline = Date.now() + milliseconds
    while (signalGroup(child, 0) && Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 50))
    }
    return !signalGroup(child, 0)
  }
  if (!(await wait(5_000))) {
    signalGroup(child, 'SIGKILL')
    if (!(await wait(5_000))) {
      throw new Error(
        'Private probe process group did not exit; files retained'
      )
    }
  }
}

async function runOwned(
  executable: string,
  args: ReadonlyArray<string>,
  env: NodeJS.ProcessEnv,
  cwd: string,
  timeoutMs: number,
  onStart: (child: ChildProcess) => void
): Promise<string> {
  const child = spawn(executable, [...args], {
    cwd,
    env,
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  onStart(child)
  let output = ''
  const collect = (chunk: Buffer) => {
    output = (output + chunk.toString()).slice(-32_768)
  }
  child.stdout?.on('data', collect)
  child.stderr?.on('data', collect)
  let timeout: ReturnType<typeof setTimeout> | undefined
  try {
    const result = await Promise.race([
      new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
        (resolve, reject) => {
          child.once('error', reject)
          child.once('close', (code, signal) => resolve({ code, signal }))
        }
      ),
      new Promise<never>((_, reject) => {
        timeout = setTimeout(
          () => reject(new Error(`${executable} timed out: ${output}`)),
          timeoutMs
        )
      }),
    ])
    if (result.code !== 0 || result.signal !== null) {
      throw new Error(
        `${executable} failed (${result.signal ?? result.code}): ${output}`
      )
    }
    return output
  } finally {
    if (timeout !== undefined) clearTimeout(timeout)
    await stopGroup(child)
  }
}

describe.skipIf(!enabled || process.platform !== 'linux')(
  'Wry Markdown sandbox',
  () => {
    it('renders links safely without executing sandboxed scripts', async () => {
      const repo = path.resolve('.')
      const root = mkdtempSync(
        path.join(tmpdir(), 'desktop-wry-markdown-test-')
      )
      const bundle = path.join(root, 'bundle')
      const target = path.join(root, 'cargo-target')
      const entry = path.join(root, 'probe.tsx')
      const output = path.join(root, 'probe-output')
      let child: ChildProcess | undefined
      let safeToDelete = true
      const env: NodeJS.ProcessEnv = Object.fromEntries(
        Object.entries(process.env).filter(
          ([key]) =>
            !key.startsWith('DESKTOP_') &&
            !key.startsWith('GIT_') &&
            ![
              'GH_TOKEN',
              'GITHUB_TOKEN',
              'SSH_AUTH_SOCK',
              'SSH_AGENT_PID',
              'GNOME_KEYRING_CONTROL',
              'DBUS_SESSION_BUS_ADDRESS',
              'DBUS_STARTER_ADDRESS',
              'DBUS_STARTER_BUS_TYPE',
              'WAYLAND_DISPLAY',
            ].includes(key)
        )
      )
      for (const name of [
        'user',
        'config',
        'data',
        'cache',
        'runtime',
        'config-dirs',
        'data-dirs',
      ]) {
        mkdirSync(path.join(root, name), { mode: 0o700 })
      }
      Object.assign(env, {
        NODE_ENV: 'development',
        RELEASE_CHANNEL: 'development',
        DESKTOP_VITE_DIST: bundle,
        CARGO_TARGET_DIR: target,
        DESKTOP_USER_DATA: path.join(root, 'user'),
        XDG_CONFIG_HOME: path.join(root, 'config'),
        XDG_DATA_HOME: path.join(root, 'data'),
        XDG_CACHE_HOME: path.join(root, 'cache'),
        XDG_RUNTIME_DIR: path.join(root, 'runtime'),
        XDG_CONFIG_DIRS: path.join(root, 'config-dirs'),
        XDG_DATA_DIRS: path.join(root, 'data-dirs'),
        DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(root, 'no-dbus')}`,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
        GDK_BACKEND: 'x11',
        WEBKIT_DISABLE_COMPOSITING_MODE: '1',
        WEBKIT_DISABLE_DMABUF_RENDERER: '1',
        LIBGL_ALWAYS_SOFTWARE: '1',
        NO_AT_BRIDGE: '1',
      })
      const run = async (
        command: string,
        args: ReadonlyArray<string>,
        timeout: number
      ) => {
        safeToDelete = false
        try {
          return await runOwned(command, args, env, repo, timeout, process => {
            child = process
          })
        } finally {
          if (child !== undefined) await stopGroup(child)
          child = undefined
          safeToDelete = true
        }
      }
      try {
        await run('pnpm', ['vite:build'], 180_000)
        await run(
          'cargo',
          ['build', '--locked', '-p', 'desktop-app'],
          1_200_000
        )

        const react = path.join(repo, 'app/node_modules/react')
        const reactDOM = path.join(repo, 'app/node_modules/react-dom')
        const component = path.join(
          repo,
          'app/src/ui/lib/sandboxed-markdown.tsx'
        )
        writeFileSync(
          entry,
          `
import React from ${JSON.stringify(react)}
import ReactDOM from ${JSON.stringify(reactDOM)}
import { SandboxedMarkdown } from ${JSON.stringify(component)}
const clicked: string[] = []
let done = false
const fail = (message: string) => { if (!done) { done = true; console.error('MARKDOWN_PROBE_FAILURE ' + message); window.ipc.postMessage('desktop-smoke:error') } }
setTimeout(() => fail('render timed out'), 15000)
const onParsed = () => {
  try {
    const frame = document.querySelector('iframe')
    const doc = frame?.contentDocument
    if (!doc || !doc.body.textContent?.includes('Probe heading')) throw new Error('markdown body missing')
    if (doc.querySelector('script, [onerror]')) throw new Error('unsafe markup survived sanitization')
    const links = Array.from(doc.querySelectorAll('a'))
    const link = links.find(a => a.textContent === 'test link')
    const relative = links.find(a => a.textContent === 'relative')
    if (!link || !relative) throw new Error('missing links')
    link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    relative.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    if (clicked.length !== 1 || clicked[0] !== 'https://example.invalid/docs') throw new Error('link routing failed: ' + JSON.stringify(clicked))
    const script = doc.createElement('script')
    script.textContent = 'parent.__markdownSandboxViolation = true'
    doc.body.appendChild(script)
    const externalScript = doc.createElement('script')
    externalScript.src = 'data:text/javascript,parent.__markdownSandboxViolation%3Dtrue'
    doc.body.appendChild(externalScript)
    const image = doc.createElement('img')
    image.setAttribute('onerror', 'parent.__markdownSandboxViolation=true')
    image.src = 'data:image/png;base64,invalid'
    doc.body.appendChild(image)
    setTimeout(() => {
      if (Reflect.get(window, '__markdownSandboxViolation')) return fail('iframe executed script')
      done = true
      console.error('MARKDOWN_PROBE_SUCCESS')
      window.ipc.postMessage('desktop-smoke:ok')
    }, 1000)
  } catch (error) { fail(String(error)) }
}
ReactDOM.render(React.createElement(SandboxedMarkdown, {
  markdown: ${JSON.stringify(
    '# Probe heading\n\n[test link](https://example.invalid/docs)\n\n[relative](/relative)\n\n<img src=x onerror="parent.__markdownSandboxViolation=true"><script>parent.__markdownSandboxViolation=true</script>'
  )},
  emoji: new Map(), underlineLinks: true, title: 'Markdown probe', onMarkdownParsed: onParsed,
  onMarkdownLinkClicked: (url: string) => clicked.push(url),
}), document.getElementById('probe'))
`
        )
        await build(
          mergeConfig(rendererConfig, {
            logLevel: 'silent',
            build: {
              outDir: output,
              emptyOutDir: true,
              lib: {
                entry,
                formats: ['iife'],
                name: 'MarkdownProbe',
                fileName: () => 'probe.js',
              },
              rollupOptions: {
                input: entry,
                output: { inlineDynamicImports: true },
              },
            },
          })
        )
        copyFileSync(
          path.join(output, 'probe.js'),
          path.join(bundle, 'probe.js')
        )
        writeFileSync(
          path.join(bundle, 'index.html'),
          '<!doctype html><html><body><div id="probe"></div><script src="./probe.js"></script></body></html>'
        )
        // Debug hosts read their own Vite output from disk; no shared output or
        // bind mount is changed, and --smoke-test provides a bounded exit.
        const result = await run(
          'xvfb-run',
          ['-a', path.join(target, 'debug/github-desktop-rs'), '--smoke-test'],
          45_000
        )
        assert.match(result, /MARKDOWN_PROBE_SUCCESS/)
      } finally {
        if (child !== undefined) {
          await stopGroup(child)
          safeToDelete = true
        }
        if (safeToDelete) rmSync(root, { recursive: true, force: true })
        else console.error(`Private Wry probe retained at ${root}`)
      }
    }, 1_500_000)
  }
)
