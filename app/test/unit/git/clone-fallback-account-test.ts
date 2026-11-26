import assert from 'node:assert/strict'
import { afterEach, beforeEach, describe, it, vi } from 'vitest'
import type { IGitStringExecutionOptions } from '../../../src/lib/git/core'
import { clone } from '../../../src/lib/git/clone'

const observed = vi.hoisted(() => ({
  executionOptions: undefined as IGitStringExecutionOptions | undefined,
  calls: 0,
  native: vi.fn(),
}))

vi.mock('../../../src/backend/client', () => ({
  backend: () => ({
    canonicalMutationPath: async (path: string) => path,
    cloneRepository: observed.native,
    gitVersion: async () => '2.50.0',
  }),
}))
vi.mock('../../../src/lib/git/core', async importOriginal => ({
  ...(await importOriginal<typeof import('../../../src/lib/git/core')>()),
  git: async (
    _args: ReadonlyArray<string>,
    _path: string,
    _name: string,
    options: IGitStringExecutionOptions
  ) => {
    observed.calls++
    observed.executionOptions = options
  },
}))
vi.mock('../../../src/lib/git/environment', async importOriginal => ({
  ...(await importOriginal<
    typeof import('../../../src/lib/git/environment')
  >()),
  envForRemoteOperation: async () => ({}),
  envForProxy: async () => undefined,
}))

describe('clone fallback account', () => {
  const savedEnvironment: NodeJS.ProcessEnv = {}

  beforeEach(() => {
    observed.executionOptions = undefined
    observed.calls = 0
    observed.native.mockReset().mockResolvedValue(undefined)
    for (const key of ['GIT_ALLOW_PROTOCOL', 'GIT_CONFIG_PARAMETERS']) {
      savedEnvironment[key] = process.env[key]
      delete process.env[key]
    }
  })

  afterEach(() => {
    for (const key of ['GIT_ALLOW_PROTOCOL', 'GIT_CONFIG_PARAMETERS']) {
      const value = savedEnvironment[key]
      if (value === undefined) {
        delete process.env[key]
      } else {
        process.env[key] = value
      }
    }
  })

  const assertNativeClone = () => {
    assert.strictEqual(observed.calls, 0)
    assert.strictEqual(observed.native.mock.calls.length, 1)
    const input = observed.native.mock.calls[0][0]
    assert.strictEqual(input.url, 'https://github.com/owner/repo')
    assert.strictEqual(input.path, '/tmp/clone-test')
    assert.strictEqual(input.default_branch, 'main')
    const trampoline = input.trampoline
    assert.ok(trampoline !== undefined)
    assert.ok(Number.isInteger(trampoline.desktop_port))
    assert.notStrictEqual(trampoline.trampoline_token, '')
    assert.match(trampoline.user_agent, /^git\//)
    assert.ok(
      trampoline.config_parameters.endsWith(
        `'credential.helper=' 'credential.helper=desktop'`
      )
    )
  }

  it('uses the typed host operation with a fallback account', async () => {
    const account = { endpoint: 'https://api.github.com', login: 'bob' }
    await clone('https://github.com/owner/repo', '/tmp/clone-test', {
      defaultBranch: 'main',
      fallbackAccount: account,
    })
    assertNativeClone()
  })

  it('uses the typed host operation for an unassigned clone', async () => {
    await clone('https://github.com/owner/repo', '/tmp/clone-test', {
      defaultBranch: 'main',
    })
    assertNativeClone()
  })
})
