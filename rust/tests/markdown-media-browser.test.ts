// @vitest-environment node
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { chromium } from 'playwright'
import { build } from 'vite'
import { describe, it } from 'vitest'

describe.skipIf(process.env.DESKTOP_TEST_MARKDOWN_MEDIA !== '1')(
  'Markdown media requests in Chromium',
  () => {
    it('removes an untrusted video before it can initiate a request', async () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-markdown-media-'))
      const entry = path.join(root, 'entry.ts')
      const output = path.join(root, 'output')
      let controlRequests = 0
      let rejectedRequests = 0
      const server = createServer((request, response) => {
        if (request.url === '/control.mp4') {
          controlRequests++
        }
        if (request.url?.startsWith('/rejected')) {
          rejectedRequests++
        }
        if (request.url?.endsWith('.mp4') || request.url?.endsWith('.vtt')) {
          response.writeHead(204).end()
        } else if (request.url === '/probe.js') {
          response.writeHead(200, { 'content-type': 'application/javascript' })
          response.end(readFileSync(path.join(output, 'probe.js')))
        } else {
          response.writeHead(200, { 'content-type': 'text/html' })
          response.end('<!doctype html><script src="/probe.js"></script>')
        }
      })
      let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined
      try {
        writeFileSync(
          entry,
          `import { sanitizeMarkdownBody } from ${JSON.stringify(
            path.resolve('app/src/ui/lib/sanitize-markdown-body')
          )}; Reflect.set(window, 'sanitizeFixture', sanitizeMarkdownBody)`
        )
        await build({
          configFile: false,
          logLevel: 'error',
          build: {
            outDir: output,
            emptyOutDir: true,
            rollupOptions: {
              input: entry,
              output: {
                format: 'iife',
                entryFileNames: 'probe.js',
                inlineDynamicImports: true,
              },
            },
          },
        })
        await new Promise<void>((resolve, reject) => {
          server.once('error', reject)
          server.listen(0, '127.0.0.1', resolve)
        })
        const address = server.address()
        assert.ok(address !== null && typeof address !== 'string')
        const base = `http://127.0.0.1:${address.port}`
        browser = await chromium.launch({ headless: true })
        const page = await browser.newPage()
        await page.route(
          'https://user-images.githubusercontent.com/**',
          route => route.abort()
        )
        await page.goto(base)
        // Confirm this browser really requests an unsanitized media URL.
        await page.evaluate(url => {
          const frame = document.createElement('iframe')
          frame.sandbox.add('allow-same-origin', 'allow-scripts')
          document.body.appendChild(frame)
          const doc = frame.contentDocument
          if (doc === null) {
            throw new Error('The control frame is unavailable')
          }
          doc.open()
          doc.write(
            `<video preload="metadata" src="${url}/control.mp4"></video>`
          )
          doc.close()
        }, base)
        const deadline = Date.now() + 5_000
        while (controlRequests === 0 && Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 20))
        }
        assert.ok(
          controlRequests > 0,
          'The unsanitized control video did not request media'
        )
        const sanitized = await page.evaluate(url => {
          const sanitize = Reflect.get(window, 'sanitizeFixture')
          if (typeof sanitize !== 'function') {
            throw new Error('Missing sanitizer')
          }
          const html: string = sanitize(
            `<video preload="metadata" src="${url}/rejected.mp4"></video>
             <video preload="metadata" src="https://user-images.githubusercontent.com/fixture.mp4">
               <source src="${url}/rejected-source.mp4">
               <track default src="${url}/rejected-track.vtt">
             </video>`
          )
          const frame = document.createElement('iframe')
          frame.sandbox.add('allow-same-origin', 'allow-scripts')
          document.body.appendChild(frame)
          const doc = frame.contentDocument
          if (doc === null) {
            throw new Error('The sanitized frame is unavailable')
          }
          doc.open()
          doc.write(html)
          doc.close()
          return html
        }, base)
        assert.match(sanitized, /user-images\.githubusercontent\.com/)
        assert.doesNotMatch(sanitized, /rejected/)
        await new Promise(resolve => setTimeout(resolve, 1_000))
        assert.equal(
          rejectedRequests,
          0,
          'An untrusted video initiated a request'
        )
      } finally {
        if (browser) {
          await browser.close()
        }
        await new Promise<void>(resolve => server.close(() => resolve()))
        rmSync(root, { recursive: true, force: true })
      }
    }, 30_000)
  }
)
