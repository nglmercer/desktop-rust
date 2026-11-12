import { afterEach, describe, expect, it, vi } from 'vitest'

// The store only formats supplied conflict data here; Git context discovery is
// outside this test and pulls in native Electron-only packages.
vi.mock('../../src/lib/copilot-conflict-context', () => ({
  formatConflictContextForPrompt: () => '',
}))

import { CopilotStore } from '../../src/lib/stores/copilot-store'
import { Account } from '../../src/models/account'

describe('Copilot generation during backend migration', () => {
  afterEach(() => {
    Reflect.deleteProperty(window, '__desktop')
    vi.restoreAllMocks()
  })

  it('resolves the selected built-in model and its supported effort for Rust', async () => {
    const account = new Account(
      'octocat',
      'https://api.github.com',
      'token',
      [],
      '',
      1,
      'Octocat'
    )
    const accounts = {
      onDidUpdate: vi.fn(),
      getAll: vi.fn(async () => [account]),
    }
    const store = new CopilotStore(accounts as never)
    vi.spyOn(store as any, 'getCachedModels').mockResolvedValue([
      { id: 'selected-model', supportedReasoningEfforts: ['high', 'medium'] },
    ])
    await expect(
      store.resolveCommitMessageModel(account, {
        kind: 'copilot',
        modelId: 'selected-model',
      })
    ).resolves.toEqual({ modelId: 'selected-model', reasoningEffort: 'medium' })
  })

  it('retains the selected account, repository and provider when Wry is present', async () => {
    const invoke = vi.fn(async () => {
      throw new Error('unexpected backend call')
    })
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke, on: vi.fn(), send: vi.fn() },
    })
    const account = new Account(
      'octocat',
      'https://api.github.com',
      'token',
      [],
      '',
      1,
      'Octocat'
    )
    const accounts = {
      onDidUpdate: vi.fn(),
      getAll: vi.fn(async () => [account]),
      createTokenGetter: vi.fn(() => async () => ({
        accessToken: account.token,
      })),
      isRefreshable: vi.fn(() => false),
    }
    const store = new CopilotStore(accounts as never)
    const client = { stop: vi.fn(async () => []) }
    const createClient = vi
      .spyOn(store as any, 'createClient')
      .mockResolvedValue(client)
    const stopped = new Error('session captured')
    const createSession = vi
      .spyOn(store as any, 'createCancellableSession')
      .mockRejectedValue(stopped)
    const provider = {
      type: 'openai',
      baseUrl: 'https://example.invalid',
    } as const

    await expect(
      store.generateCommitMessage(account, 'diff', '/repo-a', {
        kind: 'byok',
        modelId: 'chosen-model',
        provider,
      })
    ).rejects.toBe(stopped)
    expect(invoke).not.toHaveBeenCalled()
    expect(createClient.mock.calls[0][0]).toBe(account)
    expect(createClient.mock.calls[0][1]).toBe('/repo-a')
    expect(createSession.mock.calls[0][1]).toMatchObject({
      model: 'chosen-model',
      provider,
    })
  })
})
