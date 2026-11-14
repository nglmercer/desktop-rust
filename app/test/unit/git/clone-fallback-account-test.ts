import assert from 'node:assert/strict'
import { beforeEach, describe, it, vi } from 'vitest'
import type { IGitStringExecutionOptions } from '../../../src/lib/git/core'
import { clone } from '../../../src/lib/git/clone'

const observed = vi.hoisted(() => ({
  executionOptions: undefined as IGitStringExecutionOptions | undefined,
  calls: 0,
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
  ...(await importOriginal<typeof import('../../../src/lib/git/environment')>()),
  envForRemoteOperation: async () => ({}),
}))

describe('clone fallback account', () => {
  beforeEach(() => {
    observed.executionOptions = undefined
    observed.calls = 0
  })

  it('passes the fallback account as an execution option', async () => {
    const account = { endpoint: 'https://api.github.com', login: 'bob' }
    await clone('https://github.com/owner/repo', '/tmp/clone-test', {
      defaultBranch: 'main',
      fallbackAccount: account,
    })
    assert.strictEqual(observed.calls, 1)
    assert.deepStrictEqual(observed.executionOptions?.fallbackAccount, account)
  })

  it('omits the fallback account for an unassigned clone', async () => {
    await clone('https://github.com/owner/repo', '/tmp/clone-test', {
      defaultBranch: 'main',
    })
    assert.strictEqual(observed.calls, 1)
    assert.strictEqual(observed.executionOptions?.fallbackAccount, undefined)
  })
})
