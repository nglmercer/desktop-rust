import { afterEach, describe, expect, it, vi } from 'vitest'

const client = vi.hoisted(() => ({
  transportKind: 'wry',
  parseWindowsIntegrationArguments: vi.fn(),
  validateCustomIntegrationPath: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => client }))

import {
  parseCustomIntegrationArgumentsForInput,
  parseCustomIntegrationArgumentsForRuntime,
  spawnCustomIntegration,
  validateCustomIntegrationPath,
} from '../../src/lib/custom-integration'

describe('native custom integration settings', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.clearAllMocks()
  })

  it('checks Windows input synchronously and parses it on the host before use', async () => {
    vi.stubGlobal('__WIN32__', true)
    client.parseWindowsIntegrationArguments.mockResolvedValueOnce([
      'a b',
      '%TARGET_PATH%',
    ])
    const raw = '"a b" %TARGET_PATH%'
    expect(parseCustomIntegrationArgumentsForInput(raw)).toEqual([raw])
    expect(client.parseWindowsIntegrationArguments).not.toHaveBeenCalled()
    expect(await parseCustomIntegrationArgumentsForRuntime(raw)).toEqual([
      'a b',
      '%TARGET_PATH%',
    ])
    expect(client.parseWindowsIntegrationArguments).toHaveBeenCalledWith(raw)
    expect(() =>
      parseCustomIntegrationArgumentsForInput('bad\0input')
    ).toThrow()
    expect(() =>
      parseCustomIntegrationArgumentsForInput('é'.repeat(32769))
    ).toThrow()
  })

  it('validates the selected executable on the host', async () => {
    client.validateCustomIntegrationPath.mockResolvedValueOnce({
      isValid: true,
      bundleID: 'test.bundle',
    })
    expect(await validateCustomIntegrationPath('/path/app')).toEqual({
      isValid: true,
      bundleID: 'test.bundle',
    })
    expect(client.validateCustomIntegrationPath).toHaveBeenCalledWith(
      '/path/app'
    )
  })

  it('launches custom integrations through the host process bridge', () => {
    const calls: Array<{ channel: string; options: any }> = []
    vi.stubGlobal('__desktop', {
      invoke: vi.fn((channel: string, options: any) => {
        calls.push({ channel, options })
        return Promise.resolve({ id: 1, pid: 1001 })
      }),
      send: vi.fn(),
      on: vi.fn(),
      encodeBase64: (bytes: Uint8Array) =>
        Buffer.from(bytes).toString('base64'),
      decodeBase64: (text: string) =>
        new Uint8Array(Buffer.from(text, 'base64')),
    })

    const child = spawnCustomIntegration(
      '/opt/custom/editor',
      ['/repository'],
      { cwd: '/repository' }
    )

    expect(child.stdout).toBeNull()
    expect(child.stderr).toBeNull()
    expect(calls).toEqual([
      {
        channel: 'desktop.io.process-spawn',
        options: {
          command: '/opt/custom/editor',
          args: ['/repository'],
          cwd: '/repository',
          env: undefined,
          stdio: ['ignore', 'ignore', 'ignore'],
          detached: true,
        },
      },
    ])
  })
})
