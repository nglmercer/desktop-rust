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
})
