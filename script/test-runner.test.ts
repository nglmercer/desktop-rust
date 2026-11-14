import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { it } from 'vitest'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)

it('runs legacy and Vitest suites together with ordinary CommonJS resolution', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'desktop-test-runner-'))
  try {
    const fixture = join(directory, 'legacy-test.ts')
    await writeFile(
      fixture,
      `
import assert from 'node:assert/strict'
import { writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { join } from 'node:path'
import { it } from 'node:test'
const fromApp = createRequire(join(process.cwd(), 'app/package.json'))
it('loads the actual Copilot SDK and test DOM', () => {
  assert.equal(typeof fromApp('@github/copilot-sdk').CopilotClient, 'function')
  assert.equal(typeof window.document.createElement, 'function')
  writeFileSync(process.env.DESKTOP_RUNNER_SENTINEL, 'legacy passed')
})
`
    )
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      DESKTOP_RUNNER_SENTINEL: join(directory, 'passed'),
    }
    delete env.NODE_TEST_CONTEXT
    const { stdout } = await execFileAsync(
      process.execPath,
      [
        '--import',
        'tsx',
        resolve('script/test.mts'),
        '--',
        fixture,
        resolve('app/test/unit/commit-mode-storage-test.ts'),
      ],
      { cwd: resolve('.'), timeout: 30000, env }
    )
    assert.equal(
      await readFile(join(directory, 'passed'), 'utf8'),
      'legacy passed'
    )
    assert.match(stdout, /commit-mode-storage-test\.ts/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}, 30000)
