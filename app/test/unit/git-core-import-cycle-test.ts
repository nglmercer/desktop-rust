// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

describe('Git module initialization', () => {
  it('loads the real core and fetch error import cycle in Node', () => {
    // Mock only native integrations unrelated to the cycle. Core, trampoline,
    // stores, fetch, and fetch-error load through their actual Node graph.
    // The application graph relies on CommonJS semantics (such as __dirname),
    // so load it through require like the former Node test loader did.
    const script = `
      const { mock } = require('node:test')
      globalThis.localStorage = { getItem() { return null }, setItem() {}, removeItem() {} }
      mock.module(process.cwd() + '/app/src/web/keytar.ts', { defaultExport: {} })
      const unavailable = () => { throw new Error('unexpected native notification call') }
      mock.module(
        process.cwd() + '/app/src/lib/notifications/show-notification.ts',
        { namedExports: { showNotification: unavailable } }
      )
      mock.module(
        process.cwd() + '/app/src/lib/notifications/notification-permission.ts',
        { namedExports: { getSystemNotificationsPermission: async () => 'denied' } }
      )
      const core = require(process.cwd() + '/app/src/lib/git/core.ts')
      const fetchErrors = require(process.cwd() + '/app/src/lib/git/fetch-error.ts')
      const error = fetchErrors.fetchFailureToGitError(
        { exitCode: 128, errorKind: 'https_saml_reauth_required' }, '/repo', 'origin'
      )
      if (!(error instanceof core.GitError) || !fetchErrors.isNativeFetchSamlReauthError(error)) {
        throw new Error('Native fetch errors lost their GitError or SAML identity')
      }
    `
    const directory = mkdtempSync(join(tmpdir(), 'desktop-import-cycle-'))
    const scriptPath = join(directory, 'cycle.cjs')
    writeFileSync(scriptPath, script)
    const result = spawnSync(
      process.execPath,
      [
        '--experimental-test-module-mocks',
        '--require',
        './script/ts-register.cjs',
        '--require',
        './app/test/globals.cjs',
        scriptPath,
      ],
      {
        cwd: process.cwd(),
        env: { ...process.env, TEST_ENV: '1' },
        encoding: 'utf8',
        timeout: 60_000,
      }
    )
    rmSync(directory, { recursive: true, force: true })
    expect(result.error).toBeUndefined()
    expect(result.status, result.stderr).toBe(0)
  }, 90_000)
})
