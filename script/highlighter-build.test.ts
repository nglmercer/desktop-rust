// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createContext, runInContext } from 'node:vm'
import { describe, it } from 'vitest'

describe('Vite syntax highlighter worker', () => {
  it('builds a standalone worker without deleting existing output', async () => {
    const output = mkdtempSync(path.join(tmpdir(), 'desktop-highlighter-'))
    const sentinel = path.join(output, 'other-bundle.js')
    writeFileSync(sentinel, 'preserve')
    try {
      const result = spawnSync(
        process.execPath,
        [
          'node_modules/vite/bin/vite.js',
          'build',
          '-c',
          'vite.highlighter.config.ts',
          '--outDir',
          output,
          '--sourcemap',
        ],
        { encoding: 'utf8', timeout: 60_000 }
      )
      assert.equal(result.status, 0, result.stderr)
      assert.doesNotMatch(result.stderr, /Sourcemap is likely to be incorrect/)
      assert.equal(readFileSync(sentinel, 'utf8'), 'preserve')
      const source = readFileSync(path.join(output, 'highlighter.js'), 'utf8')
      assert.ok(
        readFileSync(path.join(output, 'highlighter.js.map')).length > 0
      )
      const replies: unknown[] = []
      const context = createContext({
        onmessage: null,
        postMessage: (value: unknown) => replies.push(value),
        console,
        performance,
      })
      runInContext(source, context, { timeout: 10_000 })
      assert.equal(typeof context.onmessage, 'function')
      for (const request of [
        {
          contentLines: ['const answer = 42'],
          basename: 'example.js',
          extension: '.js',
          tabSize: 4,
          lines: [0],
          addModeClass: true,
        },
        {
          contentLines: ['def answer():', '    return 42'],
          basename: 'example.py',
          extension: '.py',
          tabSize: 4,
          lines: [0, 1],
          addModeClass: true,
        },
      ])
        await context.onmessage({ data: request })
      assert.match(JSON.stringify(replies[0]), /m-javascript keyword/)
      assert.match(JSON.stringify(replies[0]), /m-javascript number/)
      assert.match(JSON.stringify(replies[1]), /m-python keyword/)
      assert.match(JSON.stringify(replies[1]), /m-python number/)
    } finally {
      rmSync(output, { recursive: true, force: true })
    }
  }, 90_000)
})
