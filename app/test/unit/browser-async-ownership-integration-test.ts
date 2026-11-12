// @vitest-environment node
import { spawn, type ChildProcess } from 'node:child_process'
import path from 'node:path'
import { createServer } from 'node:net'
import { createInterface } from 'node:readline'
import { chromium, type Browser } from 'playwright'
import { afterEach, describe, expect, it } from 'vitest'

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
})

describe.skipIf(process.env.DESKTOP_TEST_BROWSER_OWNERSHIP !== '1')(
  'browser operation ownership',
  () => {
    it('rejects the legacy lease and loads named crypto APIs in Chromium', async () => {
      const probe = createServer()
      await new Promise<void>((resolve, reject) => {
        probe.once('error', reject)
        probe.listen(0, '127.0.0.1', resolve)
      })
      const socketAddress = probe.address()
      if (!socketAddress || typeof socketAddress === 'string')
        throw new Error('Could not select a local port')
      const port = socketAddress.port
      await new Promise<void>((resolve, reject) =>
        probe.close(error => (error ? reject(error) : resolve()))
      )
      const child = spawn(
        process.execPath,
        [
          path.resolve('node_modules/vite/bin/vite.js'),
          '--host',
          '127.0.0.1',
          '--port',
          String(port),
          '--strictPort',
        ],
        { stdio: ['ignore', 'pipe', 'pipe'] }
      )
      server = child
      child.stderr?.resume()
      if (!child.stdout) throw new Error('Vite stdout is unavailable')
      const lines = createInterface({ input: child.stdout })
      const address = await new Promise<string>((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error('Vite startup timed out')),
          15_000
        )
        lines.on('line', line => {
          const url = /Local:\s+(http:\/\/127\.0\.0\.1:\d+\/)/.exec(line)?.[1]
          if (url) {
            clearTimeout(timer)
            resolve(url)
          }
        })
        child.once('exit', () => {
          clearTimeout(timer)
          reject(new Error('Vite exited before startup'))
        })
      })
      browser = await chromium.launch({
        headless: true,
        executablePath: process.env.DESKTOP_CHROMIUM,
        args: ['--no-sandbox'],
      })
      const page = await browser.newPage()
      await page.route('**/__ownership_test__', route =>
        route.fulfill({
          status: 200,
          contentType: 'text/html',
          body: '<!doctype html>',
        })
      )
      await page.goto(`${address}__ownership_test__`)
      const result = await page.evaluate(async () => {
        Object.assign(globalThis, {
          __WIN32__: false,
          __DARWIN__: false,
          __LINUX__: true,
          __DEV__: false,
          __TEST__: true,
          __OAUTH_CLIENT_ID__: '',
          __OAUTH_SECRET__: '',
          __APP_NAME__: 'Desktop',
          __APP_VERSION__: 'test',
          __RELEASE_CHANNEL__: 'development',
          __SHA__: 'test',
          __UPDATES_URL__: '',
          log: { info() {}, warn() {}, error() {}, debug() {} },
        })
        const load = new Function('url', 'return import(url)') as (
          url: string
        ) => Promise<unknown>
        const { createRepositoryOperationContext } = (await load(
          '/src/lib/git/repository-operation-context.ts'
        )) as {
          createRepositoryOperationContext: () => {
            supportsAsyncPropagation: boolean
            getStore: () => string | undefined
            run: (
              value: string,
              operation: () => Promise<void>
            ) => Promise<void>
          }
        }
        const scope = createRepositoryOperationContext()
        let before: string | undefined
        let after: string | undefined
        await scope.run('owner', async () => {
          before = scope.getStore()
          await Promise.resolve()
          after = scope.getStore()
        })
        const { acquireAssistedCommitGitLease } = (await load(
          '/src/lib/git/repository-operation.ts'
        )) as {
          acquireAssistedCommitGitLease: (path: string) => Promise<unknown>
        }
        let code: string | undefined
        try {
          await acquireAssistedCommitGitLease('/repo')
        } catch (error) {
          code = (error as { code?: string }).code
        }
        const { executeAssistedCommitRun } = (await load(
          '/src/lib/assisted-commit/execution.ts'
        )) as {
          executeAssistedCommitRun: (
            run: unknown,
            request: unknown,
            prepared: unknown,
            services: unknown
          ) => Promise<unknown>
        }
        let startCode: string | undefined
        try {
          await executeAssistedCommitRun(null, null, undefined, null)
        } catch (error) {
          startCode = (error as { code?: string }).code
        }
        const crypto = (await load('/src/web/crypto.ts')) as {
          createHash: (name: string) => {
            update: (text: string) => { digest: (encoding: string) => string }
          }
          randomBytes: (length: number) => { length: number }
        }
        const streams = (await load('/src/web/stream.ts')) as {
          Transform: typeof import('node:stream').Transform
        }
        const transform = new streams.Transform({
          transform(chunk, _encoding, callback) {
            callback(null, chunk)
          },
        })
        let streamed = ''
        await new Promise<void>((resolve, reject) => {
          transform.on('data', (chunk: Buffer) => {
            streamed += chunk.toString('utf8')
          })
          transform.on('error', reject)
          transform.on('end', resolve)
          transform.end('stream works')
        })
        return {
          supported: scope.supportsAsyncPropagation,
          before,
          after: after ?? null,
          code,
          startCode,
          hash: crypto.createHash('sha256').update('abc').digest('hex'),
          randomLength: crypto.randomBytes(4).length,
          streamed,
        }
      })
      expect(result).toEqual({
        supported: false,
        before: 'owner',
        after: null,
        code: 'unsupported-runtime',
        startCode: 'unsupported-runtime',
        hash: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
        randomLength: 4,
        streamed: 'stream works',
      })
    }, 60_000)
  }
)
