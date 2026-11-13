// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  existsSync,
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

// Requires a debug desktop-app binary, Xvfb, and Linux user/mount namespaces.
// Explicit opt-in makes missing host GUI prerequisites a failure rather than
// silently reporting this integration coverage on an unsupported machine.
const enabled = process.env.DESKTOP_WRY_WEBVIEW_TEST === '1'

/** Terminate the private Xvfb/Wry process group before deleting its files. */
async function stopProcessGroup(pid: number, closed: Promise<void>) {
  const signal = (name: NodeJS.Signals) => {
    try {
      process.kill(-pid, name)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
    }
  }
  const waitForClose = async (milliseconds: number) => {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        closed.then(() => true),
        new Promise<false>(resolve => {
          timer = setTimeout(() => resolve(false), milliseconds)
        }),
      ])
    } finally {
      if (timer !== undefined) clearTimeout(timer)
    }
  }
  signal('SIGTERM')
  if (!(await waitForClose(5_000))) {
    signal('SIGKILL')
    if (!(await waitForClose(5_000))) {
      throw new Error('Wry process group did not close')
    }
  }
}

describe.skipIf(!enabled || process.platform !== 'linux')(
  'Wry renderer bundled emoji',
  () => {
    it('loads gemoji and renders an image through the real desktop protocol', async () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-wry-emoji-test-'))
      const output = path.join(root, 'bundle')
      const home = path.join(root, 'home')
      const entry = path.join(root, 'probe.ts')
      const binary = path.resolve('rust/target/debug/github-desktop-rs')
      const mountpoint = path.resolve('out/vite-dist')
      let safeToDelete = true
      try {
        assert.ok(
          existsSync(binary),
          'Build the debug desktop-app binary first'
        )
        assert.ok(existsSync(mountpoint), 'Build the web renderer first')
        for (const directory of ['config', 'data', 'cache', 'runtime']) {
          mkdirSync(path.join(home, directory), {
            recursive: true,
            mode: 0o700,
          })
        }
        const filter = path.resolve('app/src/lib/markdown-filters/emoji-filter')
        const reader = path.resolve('app/src/lib/read-emoji')
        writeFileSync(
          entry,
          `
import { EmojiFilter } from ${JSON.stringify(filter)}
import { readEmoji } from ${JSON.stringify(reader)}
;(window as any).runProbe = async () => {
  const emojis = await readEmoji('/__desktop_app__')
  const shipit = emojis.get(':shipit:')
  if (!shipit) throw new Error('Missing shipit in gemoji database')
  const nodes = await new EmojiFilter(emojis).filter(document.createTextNode('hello :shipit:'))
  const image = nodes?.find(node => node instanceof HTMLImageElement) as HTMLImageElement | undefined
  if (!image || !image.src.startsWith('data:image/png;base64,iVBOR')) throw new Error('Emoji filter did not render a PNG')
  return { url: shipit.url, imagePrefix: image.src.slice(0, 45) }
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
                name: 'WryEmojiProbe',
                fileName: () => 'probe.js',
              },
            },
          })
        )
        writeFileSync(
          path.join(output, 'index.html'),
          `<!doctype html><meta charset="UTF-8">
<script src="./probe.js"></script>
<script>window.runProbe().then(
  value => console.error('WRY_EMOJI_PROBE_SUCCESS ' + JSON.stringify(value)),
  error => console.error('WRY_EMOJI_PROBE_FAILURE ' + String(error && (error.stack || error)))
)</script>`
        )

        // The debug shell uses its build-time repository path for assets. A
        // private mount namespace supplies the fixture without modifying that
        // shared output directory, even if the test fails or is interrupted.
        const shell = 'mount --bind "$1" "$2" || exit; exec xvfb-run -a "$3"'
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
            shell,
            'sh',
            output,
            mountpoint,
            binary,
          ],
          {
            detached: true,
            env: {
              ...process.env,
              HOME: home,
              XDG_CONFIG_HOME: path.join(home, 'config'),
              XDG_DATA_HOME: path.join(home, 'data'),
              XDG_CACHE_HOME: path.join(home, 'cache'),
              XDG_RUNTIME_DIR: path.join(home, 'runtime'),
              WEBKIT_DISABLE_COMPOSITING_MODE: '1',
              NO_AT_BRIDGE: '1',
            },
            stdio: ['ignore', 'pipe', 'pipe'],
          }
        )
        safeToDelete = false
        // Register this immediately, before waiting for output, so fast exit
        // and spawn errors cannot leave cleanup waiting for a missed event.
        const closed = new Promise<void>(resolve => {
          child.once('close', () => resolve())
        })
        try {
          const message = await new Promise<string>((resolve, reject) => {
            let text = ''
            const timeout = setTimeout(
              () => reject(new Error(`Wry probe timed out: ${text}`)),
              45_000
            )
            const collect = (chunk: Buffer) => {
              text = (text + chunk.toString()).slice(-32_768)
              const result = text.match(
                /WRY_EMOJI_PROBE_(SUCCESS|FAILURE) ([^\r\n]*)/
              )
              if (result) {
                clearTimeout(timeout)
                if (result[1] === 'SUCCESS') resolve(result[2])
                else reject(new Error(`Wry emoji probe failed: ${result[2]}`))
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
          const result: unknown = JSON.parse(message)
          assert.deepEqual(result, {
            url: 'desktop://localhost/emoji/shipit.png',
            imagePrefix: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgA',
          })
        } finally {
          if (child.pid !== undefined) {
            await stopProcessGroup(child.pid, closed)
          } else {
            await closed
          }
          safeToDelete = true
        }
      } finally {
        if (safeToDelete) rmSync(root, { recursive: true, force: true })
      }
    }, 90_000)
  }
)
