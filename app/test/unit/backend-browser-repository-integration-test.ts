// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { chromium, type Browser } from 'playwright'
import { afterEach, describe, expect, it } from 'vitest'

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
  'browser repository workflow',
  () => {
    it('adds a repository and binds the Rust bridge before reading its changes', async () => {
      directory = await mkdtemp(path.join(tmpdir(), 'desktop-browser-repo-'))
      const repository = path.join(directory, 'repository')
      execFileSync('git', ['init', '-q', repository])
      await writeFile(path.join(repository, 'changed.txt'), 'initial\n')
      execFileSync('git', ['-C', repository, 'add', 'changed.txt'])
      execFileSync('git', [
        '-C',
        repository,
        '-c',
        'user.name=Test',
        '-c',
        'user.email=test@example.invalid',
        '-c',
        'commit.gpgsign=false',
        'commit',
        '-qm',
        'initial',
      ])
      await writeFile(path.join(repository, 'changed.txt'), 'modified\n')
      const config = path.join(directory, 'gitconfig')
      await writeFile(
        config,
        '[user]\n name = Browser Test\n email = browser@example.invalid\n'
      )
      server = spawn(
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
            GIT_CONFIG_GLOBAL: config,
            GIT_CONFIG_NOSYSTEM: '1',
          },
          stdio: ['ignore', 'ignore', 'pipe'],
        }
      )
      const child = server
      if (!child.stderr) throw new Error('Missing bridge output')
      const lines = createInterface({ input: child.stderr })
      const announcement = await new Promise<string>((resolve, reject) => {
        lines.once('line', resolve)
        child.once('error', reject)
        child.once('exit', () =>
          reject(new Error('Bridge stopped before startup'))
        )
      })
      const session = /^Open (http:\/\/127\.0\.0\.1:\d+\/#token=\S+)$/.exec(
        announcement
      )?.[1]
      if (!session) throw new Error('Invalid bridge startup announcement')
      browser = await chromium.launch({
        headless: true,
        executablePath: process.env.DESKTOP_CHROMIUM,
        args: ['--no-sandbox'],
      })
      const page = await browser.newPage()
      await page.route('https://**', route => route.abort())
      await page.addInitScript(() => {
        localStorage.setItem('has-shown-welcome-flow', '1')
        const calls: string[] = []
        Reflect.set(window, '__browserAudit', calls)
        const send = WebSocket.prototype.send
        WebSocket.prototype.send = function (value) {
          try {
            const request = JSON.parse(String(value))
            if (typeof request.method === 'string')
              calls.push(
                JSON.stringify({
                  method: request.method,
                  args:
                    request.method === 'desktop.io.git-exec'
                      ? request.args
                      : undefined,
                })
              )
          } catch {}
          return send.call(this, value)
        }
      })
      const errors: string[] = []
      page.on('pageerror', error =>
        errors.push(
          error.message.replace(/#token=[^\s]+/g, '#token=[redacted]')
        )
      )
      try {
        await page.goto(session, { waitUntil: 'domcontentloaded' })
      } catch {
        throw new Error('Could not open isolated browser frontend')
      }
      try {
        await page
          .getByText(/Add an Existing Repository from your local drive/i)
          .click({ timeout: 20_000 })
        await page.getByPlaceholder('repository path').fill(repository)
        await page.getByRole('button', { name: /Add repository/i }).click()
        await page
          .getByText('changed.txt', { exact: true })
          .first()
          .waitFor({ timeout: 20_000 })
      } catch {
        const calls = await page.evaluate(
          () => Reflect.get(window, '__browserAudit') as string[]
        )
        throw new Error(
          `Repository journey did not reach the changes list: ${((await page.locator('body').textContent()) ?? '').slice(0, 2000)}; errors: ${errors.join('; ')}; calls: ${calls.filter(call => !call.includes('"method":"path_exists"')).join(' ')}`
        )
      }
      const calls = await page.evaluate(
        () => Reflect.get(window, '__browserAudit') as string[]
      )
      expect(
        calls.some(call => JSON.parse(call).method === 'open_repository')
      ).toBe(true)
      expect(
        calls.some(call => JSON.parse(call).method === 'status_detailed')
      ).toBe(true)
    }, 90_000)
  }
)
