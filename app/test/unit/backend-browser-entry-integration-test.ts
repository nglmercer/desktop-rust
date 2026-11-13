// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { chromium, type Browser } from 'playwright'
import { afterEach, describe, it, vi } from 'vitest'

let directory: string | undefined
let server: ChildProcess | undefined
let browser: Browser | undefined
afterEach(async () => {
  await browser?.close()
  browser = undefined
  if (server && server.exitCode === null && server.signalCode === null) {
    const child = server
    const stopped = new Promise<void>(resolve =>
      child.once('exit', () => resolve())
    )
    child.kill('SIGINT')
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        stopped,
        new Promise<void>(resolve => {
          timer = setTimeout(resolve, 5000)
        }),
      ])
    } finally {
      if (timer) clearTimeout(timer)
    }
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGKILL')
      await stopped
    }
  }
  server = undefined
  if (directory) await rm(directory, { recursive: true, force: true })
  directory = undefined
})

describe.skipIf(process.env.DESKTOP_TEST_BROWSER_ENTRY !== '1')(
  'embedded browser frontend',
  () => {
    it('mounts against the real authenticated loopback bridge without legacy identity calls', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-browser-entry-'))
      const gitConfig = path.join(directory, 'gitconfig')
      await writeFile(
        gitConfig,
        '[user]\n name = Browser Test\n email = browser@example.invalid\n'
      )
      const child = spawn(
        path.resolve('rust/target/debug/desktop-rs'),
        [
          '--config',
          path.join(directory, 'registry.json'),
          'serve',
          '--empty',
          '--port',
          '0',
        ],
        {
          env: {
            ...process.env,
            HOME: directory,
            GIT_CONFIG_GLOBAL: gitConfig,
            GIT_CONFIG_NOSYSTEM: '1',
          },
          stdio: ['ignore', 'ignore', 'pipe'],
        }
      )
      server = child
      assert.ok(child.stderr)
      const lines = createInterface({ input: child.stderr })
      const announcement = await new Promise<string>((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error('bridge startup timed out')),
          15_000
        )
        lines.once('line', line => {
          clearTimeout(timer)
          resolve(line)
        })
        child.once('exit', () => {
          clearTimeout(timer)
          reject(new Error('bridge exited before startup'))
        })
      })
      const session = /^Open (http:\/\/127\.0\.0\.1:\d+\/#token=\S+)$/.exec(
        announcement
      )?.[1]
      assert.ok(session, 'Invalid bridge startup announcement')
      browser = await chromium.launch({
        headless: true,
        executablePath: process.env.DESKTOP_CHROMIUM,
        args: ['--no-sandbox'],
      })
      const page = await browser.newPage()
      await page
        .context()
        .grantPermissions(['clipboard-read', 'clipboard-write'], {
          origin: new URL(session).origin,
        })
      await page.route('https://**', route => route.abort())
      await page.addInitScript(() => {
        const captured: string[] = []
        Reflect.set(window, '__browserAudit', captured)
        const original = WebSocket.prototype.send
        WebSocket.prototype.send = function (value) {
          try {
            const request = JSON.parse(String(value))
            if (typeof request.method === 'string')
              captured.push(
                request.method === 'desktop.io.git-exec'
                  ? `${request.method}:${JSON.stringify(request.args)}`
                  : request.method
              )
          } catch {}
          return original.call(this, value)
        }
      })
      const errors: string[] = []
      page.on('pageerror', error => errors.push(error.message))
      try {
        await page.goto(session, { waitUntil: 'domcontentloaded' })
      } catch {
        // The session URL contains the bridge credential; do not surface it
        // in Playwright's navigation error.
        throw new Error('Could not open the isolated browser frontend')
      }
      try {
        await page.waitForSelector('#desktop-app-container > *', {
          timeout: 20_000,
        })
      } catch {
        const diagnostics = errors.map(error =>
          error.replace(/#token=[^\s]+/g, '#token=[redacted]')
        )
        throw new Error(
          `Browser frontend did not mount: ${diagnostics.join('; ') || 'no page error was reported'}`
        )
      }
      await page.waitForFunction(
        () => Reflect.get(window, '__desktop')?.__backendReady === true
      )
      const clipboard = await page.evaluate(async () => {
        await Reflect.get(window, '__desktop').invoke(
          'write-clipboard-text',
          'browser clipboard test'
        )
        return navigator.clipboard.readText()
      })
      assert.equal(clipboard, 'browser clipboard test')
      const popupEvent = page.waitForEvent('popup')
      const opened = await page.evaluate(() =>
        Reflect.get(window, '__desktop').invoke(
          'open-external',
          'https://example.invalid/'
        )
      )
      const popup = await popupEvent
      assert.equal(opened, true)
      await popup.close()
      await page.waitForFunction(() =>
        (Reflect.get(window, '__browserAudit') as string[]).includes(
          'git_settings'
        )
      )
      await page.waitForFunction(() =>
        (Reflect.get(window, '__browserAudit') as string[]).includes(
          'lfs_tool_status'
        )
      )
      if (
        spawnSync('git', ['lfs', 'version'], { stdio: 'ignore' }).status === 0
      ) {
        await page.waitForFunction(() =>
          (Reflect.get(window, '__browserAudit') as string[]).includes(
            'lfs_install'
          )
        )
        await vi.waitFor(
          async () => {
            assert.match(await readFile(gitConfig, 'utf8'), /\[filter "lfs"\]/)
          },
          { timeout: 10_000 }
        )
      }
      await page.waitForTimeout(1000)
      const calls = await page.evaluate(
        () => Reflect.get(window, '__browserAudit') as string[]
      )
      assert.ok(calls.includes('git_settings'))
      assert.ok(
        !calls.some(call => call.startsWith('desktop.io.git-exec:')),
        calls.filter(call => call.startsWith('desktop.io.git-exec:')).join(', ')
      )
      assert.ok(!calls.includes('get-app-path'))
      assert.ok(!calls.includes('write-clipboard-text'))
      assert.ok(!calls.includes('open-external'))
      assert.ok(!errors.includes('invalid typed repository request'))
    }, 60_000)
  }
)
