// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { describe, expect, it } from 'vitest'

describe('Git module initialization', () => {
  it('loads the real core and fetch error import cycle in Node', () => {
    // Mock only native integrations unrelated to the cycle. Core, trampoline,
    // stores, fetch, and fetch-error load through their actual Node/tsx graph.
    const script = `
      import { mock } from 'node:test'
      import { createRequire } from 'node:module'
      import { pathToFileURL } from 'node:url'
      globalThis.localStorage = { getItem() { return null }, setItem() {}, removeItem() {} }
      const requireFromApp = createRequire(process.cwd() + '/app/package.json')
      mock.module(requireFromApp.resolve('keytar'), { defaultExport: {} })
      for (const file of [
        'app/src/lib/notifications/show-notification.ts',
        'app/src/lib/notifications/notification-permission.ts',
      ]) mock.module(pathToFileURL(process.cwd() + '/' + file), { namedExports: {} })
      const core = await import('./app/src/lib/git/core.ts')
      const fetchErrors = await import('./app/src/lib/git/fetch-error.ts')
      const error = fetchErrors.fetchFailureToGitError(
        { exitCode: 128, errorKind: 'https_saml_reauth_required' }, '/repo', 'origin'
      )
      if (!(error instanceof core.GitError) || !fetchErrors.isNativeFetchSamlReauthError(error)) {
        throw new Error('Native fetch errors lost their GitError or SAML identity')
      }
    `
    const result = spawnSync(process.execPath, [
      '--experimental-test-module-mocks',
      '--import', 'tsx',
      '--import', './app/test/globals.mts',
      '--input-type=module',
      '-e', script,
    ], {
      cwd: process.cwd(),
      env: { ...process.env, TEST_ENV: '1' },
      encoding: 'utf8',
      timeout: 15_000,
    })
    expect(result.error).toBeUndefined()
    expect(result.status, result.stderr).toBe(0)
  })
})
