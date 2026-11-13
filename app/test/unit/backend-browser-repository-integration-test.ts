// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { chromium, type Browser } from 'playwright'
import { afterEach, describe, expect, it, vi } from 'vitest'

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
      execFileSync('git', ['-C', repository, 'branch', 'checkout-target'])
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
        await page.waitForFunction(
          () => {
            const text =
              document.querySelector('.diff-container')?.textContent ?? ''
            return text.includes('initial') && text.includes('modified')
          },
          undefined,
          { timeout: 20_000 }
        )
      } catch {
        const calls = await page.evaluate(
          () => Reflect.get(window, '__browserAudit') as string[]
        )
        throw new Error(
          `Repository journey did not render the diff: ${((await page.locator('body').textContent()) ?? '').slice(0, 2000)}; errors: ${errors.join('; ')}; calls: ${calls.filter(call => !call.includes('"method":"path_exists"')).join(' ')}`
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
      expect(
        calls.some(call => JSON.parse(call).method === 'renderer_working_diff')
      ).toBe(true)
      await page.locator('.branch-button').click()
      await page.getByText('checkout-target', { exact: true }).first().click()
      const switchDialog = page.getByRole('dialog', { name: /switch branch/i })
      await switchDialog.getByText('Bring my changes to checkout-target').click()
      await switchDialog.getByRole('button', { name: /switch branch/i }).click()
      await switchDialog.waitFor({ state: 'hidden', timeout: 20_000 })
      await vi.waitFor(() => {
        expect(execFileSync('git', ['-C', repository, 'branch', '--show-current'], { encoding: 'utf8' }).trim()).toBe('checkout-target')
      }, { timeout: 20_000 })
      expect(await readFile(path.join(repository, 'changed.txt'), 'utf8')).toBe('modified\n')
      const checkoutCalls = await page.evaluate(() => Reflect.get(window, '__browserAudit') as string[])
      expect(checkoutCalls.some(call => JSON.parse(call).method === 'checkout_preview')).toBe(true)
      expect(checkoutCalls.some(call => JSON.parse(call).method === 'checkout')).toBe(true)
      await page.waitForFunction(() =>
        (Reflect.get(window, '__browserAudit') as string[]).some(
          call => JSON.parse(call).method === 'renderer_blob_prefix'
        )
      )
      await page.waitForFunction(() =>
        (Reflect.get(window, '__browserAudit') as string[]).some(
          call => JSON.parse(call).method === 'repository_worktrees'
        )
      )
      await page.waitForFunction(() => {
        const calls = Reflect.get(window, '__browserAudit') as string[]
        return [
          'renderer_recent_branches',
          'renderer_stash_log',
          'renderer_history',
          'renderer_tag_refs',
          'renderer_commit_changes',
          'renderer_commit_diff',
        ].every(method =>
          calls.some(call => JSON.parse(call).method === method)
        )
      })
      const finalCalls = await page.evaluate(
        () => Reflect.get(window, '__browserAudit') as string[]
      )
      expect(
        finalCalls.filter(
          call => JSON.parse(call).method === 'desktop.io.git-exec'
        )
      ).toEqual([])
      expect(
        finalCalls.some(call => {
          const request = JSON.parse(call)
          return (
            request.method === 'desktop.io.git-exec' &&
            request.args?.[0] === 'show'
          )
        })
      ).toBe(false)
      expect(
        finalCalls.some(call => {
          const request = JSON.parse(call)
          return (
            request.method === 'desktop.io.git-exec' &&
            request.args?.[0] === 'log' &&
            request.args?.includes('--date=raw')
          )
        })
      ).toBe(false)
      expect(
        finalCalls.some(call => {
          const request = JSON.parse(call)
          return (
            request.method === 'desktop.io.git-exec' &&
            request.args?.[0] === 'log' &&
            request.args?.[1] === '-g'
          )
        })
      ).toBe(false)
      expect(
        finalCalls.some(call => {
          const request = JSON.parse(call)
          return (
            request.method === 'desktop.io.git-exec' &&
            request.args?.[0] === 'worktree'
          )
        })
      ).toBe(false)
      expect(
        calls.some(call => {
          const request = JSON.parse(call)
          return (
            request.method === 'desktop.io.git-exec' &&
            request.args?.[0] === 'diff'
          )
        })
      ).toBe(false)
      try {
        await page
          .getByText('changed.txt', { exact: true })
          .first()
          .click({ button: 'right' })
        await page.getByRole('menuitem', { name: /^Discard changes/i }).click()
        const dialog = page.getByRole('alertdialog')
        await dialog.getByText('changed.txt', { exact: true }).waitFor()
        await dialog.getByRole('button', { name: /^Discard changes$/i }).click()
        await page
          .getByText('Selected changes discarded.')
          .waitFor({ timeout: 20_000 })
        expect(
          await readFile(path.join(repository, 'changed.txt'), 'utf8')
        ).toBe('initial\n')
        await page
          .locator('#successful')
          .getByRole('button', { name: 'Undo', exact: true })
          .click()
        await vi.waitFor(
          async () => {
            expect(
              await readFile(path.join(repository, 'changed.txt'), 'utf8')
            ).toBe('modified\n')
          },
          { timeout: 20_000 }
        )
        await page
          .getByText('changed.txt', { exact: true })
          .first()
          .waitFor({ timeout: 20_000 })
        const afterUndo = await page.evaluate(
          () => Reflect.get(window, '__browserAudit') as string[]
        )
        for (const method of [
          'discard_batch_preview',
          'discard_batch',
          'undo_discard',
        ]) {
          expect(
            afterUndo.some(call => JSON.parse(call).method === method)
          ).toBe(true)
        }
        expect(
          afterUndo.filter(
            call => JSON.parse(call).method === 'desktop.io.git-exec'
          )
        ).toEqual([])
      } catch (error) {
        const calls = await page.evaluate(
          () => Reflect.get(window, '__browserAudit') as string[]
        )
        throw new Error(
          `Discard/undo journey failed: ${String(error)}; body: ${((await page.locator('body').textContent()) ?? '').slice(0, 2000)}; errors: ${errors.join('; ')}; calls: ${calls.filter(call => !call.includes('"method":"path_exists"')).join(' ')}`
        )
      }
      const indexBeforeCommit = await readFile(
        path.join(repository, '.git', 'index')
      )
      const headBeforeCommit = execFileSync(
        'git',
        ['-C', repository, 'rev-parse', 'HEAD'],
        { encoding: 'utf8' }
      )
      await page.getByLabel('Commit summary').fill('Review selected changes')
      await page
        .getByRole('group', { name: 'Create commit' })
        .getByRole('button', { name: /^Commit(?:\s|$)/ })
        .click()
      try {
        await page.waitForFunction(
          () =>
            (Reflect.get(window, '__browserAudit') as string[]).some(
              call => JSON.parse(call).method === 'selected_commit_preview'
            ),
          undefined,
          { timeout: 5000 }
        )
      } catch {
        const audit = await page.evaluate(
          () => Reflect.get(window, '__browserAudit') as string[]
        )
        throw new Error(
          `Commit review did not run: ${((await page.locator('body').textContent()) ?? '').slice(-3000)}; calls: ${audit.filter(call => !call.includes('"method":"path_exists"')).join(' ')}; errors: ${errors.join('; ')}`
        )
      }
      expect(await readFile(path.join(repository, '.git', 'index'))).toEqual(
        indexBeforeCommit
      )
      expect(
        execFileSync('git', ['-C', repository, 'rev-parse', 'HEAD'], {
          encoding: 'utf8',
        })
      ).toBe(headBeforeCommit)
      await page.waitForFunction(
        () =>
          (Reflect.get(window, '__browserAudit') as string[]).some(
            call => JSON.parse(call).method === 'repository_protected_resources'
          ),
        undefined,
        { timeout: 5000 }
      )
      await page.waitForFunction(
        () =>
          (Reflect.get(window, '__browserAudit') as string[]).some(
            call => JSON.parse(call).method === 'desktop.io.git-exec'
          ),
        undefined,
        { timeout: 5000 }
      )
      const legacyCalls = await page.evaluate(() =>
        (Reflect.get(window, '__browserAudit') as string[])
          .map(call => JSON.parse(call))
          .filter(call => call.method === 'desktop.io.git-exec')
      )
      // The still-legacy commit flow can stop at hook discovery or index reset.
      expect(legacyCalls.length).toBeGreaterThan(0)
    }, 90_000)
  }
)
