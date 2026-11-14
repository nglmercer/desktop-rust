// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { chromium, type Browser } from 'playwright'
import { afterEach, describe, expect, it } from 'vitest'

let directory: string | undefined
let server: ChildProcess | undefined
let browser: Browser | undefined
afterEach(async () => {
  let browserError: unknown
  if (browser) {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        browser.close(),
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () => reject(new Error('Browser did not close')),
            5000
          )
        }),
      ])
    } catch (error) {
      browserError = error
    } finally {
      if (timer) clearTimeout(timer)
    }
  }
  browser = undefined
  if (
    server?.pid !== undefined &&
    server.exitCode === null &&
    server.signalCode === null
  ) {
    const child = server
    const stopped = new Promise<void>(resolve =>
      child.once('exit', () => resolve())
    )
    const wait = async (milliseconds: number): Promise<boolean> => {
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        return await Promise.race([
          stopped.then(() => true),
          new Promise<false>(resolve => {
            timer = setTimeout(() => resolve(false), milliseconds)
          }),
        ])
      } finally {
        if (timer) clearTimeout(timer)
      }
    }
    child.kill('SIGINT')
    if (!(await wait(5000))) {
      child.kill('SIGKILL')
      if (!(await wait(5000))) {
        server = undefined
        directory = undefined
        throw new Error('Bridge did not stop; temporary files were retained')
      }
    }
  }
  server = undefined
  if (directory && browserError === undefined)
    await rm(directory, { recursive: true, force: true })
  directory = undefined
  if (browserError !== undefined) throw browserError
}, 25_000)

describe.skipIf(process.env.DESKTOP_TEST_BROWSER_ENTRY !== '1')(
  'browser creation templates',
  () => {
    it('creates a repository using bundled license and C++ ignore templates over the Rust bridge', async () => {
      directory = await mkdtemp(
        path.join(tmpdir(), 'desktop-browser-templates-')
      )
      const config = path.join(directory, 'gitconfig')
      await writeFile(
        config,
        '[user]\n name = Browser Test\n email = browser@example.invalid\n'
      )
      const environment = Object.fromEntries(
        Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))
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
            ...environment,
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
        const cleanup = () => {
          clearTimeout(timer)
          lines.removeListener('line', onLine)
          child.removeListener('error', onError)
          child.removeListener('exit', onExit)
        }
        const onLine = (value: string) => {
          cleanup()
          resolve(value)
        }
        const onError = (error: Error) => {
          cleanup()
          reject(error)
        }
        const onExit = () => {
          cleanup()
          reject(new Error('Bridge stopped before startup'))
        }
        const timer = setTimeout(() => {
          cleanup()
          reject(new Error('Bridge startup timed out'))
        }, 15_000)
        lines.once('line', onLine)
        child.once('error', onError)
        child.once('exit', onExit)
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
      const serverOrigin = new URL(session).origin
      await page.route('**/*', route => {
        const url = new URL(route.request().url())
        return (url.protocol === 'http:' || url.protocol === 'https:') &&
          url.origin !== serverOrigin
          ? route.abort()
          : route.continue()
      })
      await page.addInitScript(defaultDirectory => {
        localStorage.setItem('has-shown-welcome-flow', '1')
        localStorage.setItem('last-clone-location', defaultDirectory)
      }, directory)
      const assetRequests: string[] = []
      page.on('request', request => {
        const url = new URL(request.url())
        if (url.pathname.startsWith('/static/') || url.pathname === '/__bundle')
          assetRequests.push(`${url.pathname}${url.search}`)
      })
      try {
        await page.goto(session, { waitUntil: 'domcontentloaded' })
      } catch {
        throw new Error('Could not open isolated browser frontend')
      }
      await page
        .getByText(/Create a New Repository on your local drive/i)
        .click({ timeout: 20_000 })
      const dialog = page.locator('#create-repository')
      await dialog
        .getByLabel('Git ignore')
        .locator('option[value="C++"]')
        .waitFor({ state: 'attached', timeout: 20_000 })
      await dialog
        .getByLabel('License')
        .locator('option[value="MIT License"]')
        .waitFor({ state: 'attached', timeout: 20_000 })
      await dialog.getByLabel('Git ignore').selectOption('C++')
      await dialog.getByLabel('License').selectOption('MIT License')
      await dialog.getByPlaceholder('repository name').fill('template-probe')
      await dialog.getByPlaceholder('repository path').fill(directory)
      await dialog
        .getByRole('button', { name: /Create repository/i })
        .click({ timeout: 20_000 })
      const created = path.join(directory, 'template-probe')
      await expect
        .poll(
          () =>
            readFile(path.join(created, '.gitignore'), 'utf8').catch(() => ''),
          { timeout: 30_000 }
        )
        .toContain('*.o')
      await expect
        .poll(
          () => readFile(path.join(created, 'LICENSE'), 'utf8').catch(() => ''),
          { timeout: 30_000 }
        )
        .toContain('Browser Test')
      await dialog.waitFor({ state: 'hidden', timeout: 30_000 })
      const show = (name: string) =>
        execFileSync('git', ['-C', created, 'show', `HEAD:${name}`], {
          encoding: 'utf8',
          timeout: 10_000,
          env: {
            ...environment,
            HOME: directory,
            GIT_CONFIG_GLOBAL: config,
            GIT_CONFIG_NOSYSTEM: '1',
          },
        })
      expect(show('.gitignore')).toContain('*.o')
      expect(show('LICENSE')).toContain('Browser Test')
      expect(assetRequests).toContain('/__bundle?path=static%2Fgitignore')
      expect(assetRequests).toContain('/static/available-licenses.json')
      expect(assetRequests).toContain('/static/gitignore/C%2B%2B.gitignore')
    }, 120_000)
  }
)
