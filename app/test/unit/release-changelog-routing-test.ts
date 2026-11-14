import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  changelog: vi.fn(),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    releaseChangelog: mocks.changelog,
  }),
}))

import { generateDevReleaseSummary, generateReleaseSummary, getChangeLog } from '../../src/lib/release-notes'
import { showTestUI } from '../../src/ui/lib/test-ui-components/test-ui-components'
import type { Dispatcher } from '../../src/ui/dispatcher'

const release = {
  name: 'Release',
  notes: ['[Fixed] Example'],
  pub_date: '2025-01-01T00:00:00Z',
  version: '3.0.0',
}

describe('release changelog transport', () => {
  beforeEach(() => {
    mocks.transport = 'ws'
    mocks.changelog.mockReset()
    mocks.changelog.mockResolvedValue([release])
  })

  afterEach(() => vi.unstubAllGlobals())

  it('uses the Rust bridge for browser requests without a cross-origin fetch', async () => {
    const fetcher = vi.fn()
    vi.stubGlobal('fetch', fetcher)
    vi.stubGlobal('__RELEASE_CHANNEL__', 'beta')

    await expect(getChangeLog(250)).resolves.toEqual([release])
    expect(mocks.changelog).toHaveBeenCalledWith('beta', 250)
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('does not silently retry a failed Rust request using browser fetch', async () => {
    const fetcher = vi.fn()
    vi.stubGlobal('fetch', fetcher)
    mocks.changelog.mockRejectedValue(new Error('offline'))

    await expect(getChangeLog()).rejects.toThrow('offline')
    expect(fetcher).not.toHaveBeenCalled()
  })

  it('returns no summaries when the Rust bridge has no release notes', async () => {
    mocks.changelog.mockResolvedValue([])
    await expect(generateReleaseSummary('3.0.0')).resolves.toEqual([])
    await expect(generateDevReleaseSummary()).resolves.toEqual([])
  })

  it('returns no summaries when the native changelog request is unsuccessful', async () => {
    mocks.transport = 'wry'
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false }))
    await expect(generateReleaseSummary('3.0.0')).resolves.toEqual([])
  })

  it.each(['empty', 'offline'])('reports %s release notes instead of opening an empty test popup', async failure => {
    vi.stubGlobal('__DEV__', true)
    if (failure === 'empty') mocks.changelog.mockResolvedValue([])
    else mocks.changelog.mockRejectedValue(new Error('offline'))
    const dispatcher = { postError: vi.fn(), showPopup: vi.fn() }
    await showTestUI('test-release-notes-popup', null, dispatcher as unknown as Dispatcher, new Map())
    expect(dispatcher.showPopup).not.toHaveBeenCalled()
    expect(dispatcher.postError).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining('Release notes are unavailable') }))
  })

  it('opens the test popup when release notes are available', async () => {
    vi.stubGlobal('__DEV__', true)
    vi.stubGlobal('localStorage', { getItem: () => null })
    const dispatcher = { postError: vi.fn(), showPopup: vi.fn() }
    await showTestUI('test-release-notes-popup', null, dispatcher as unknown as Dispatcher, new Map())
    expect(dispatcher.postError).not.toHaveBeenCalled()
    expect(dispatcher.showPopup).toHaveBeenCalledWith(expect.objectContaining({ newReleases: [expect.objectContaining({ latestVersion: '3.0.0' })] }))
  })

  it.each(['development', 'production'])(
    'uses the stable channel for %s',
    async channel => {
      vi.stubGlobal('__RELEASE_CHANNEL__', channel)
      await expect(getChangeLog()).resolves.toEqual([release])
      expect(mocks.changelog).toHaveBeenCalledWith(null, undefined)
    }
  )

  it('keeps the existing fetch path for the native Wry host', async () => {
    mocks.transport = 'wry'
    const fetcher = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => [release],
    })
    vi.stubGlobal('fetch', fetcher)
    vi.stubGlobal('__RELEASE_CHANNEL__', 'test')

    await expect(getChangeLog(2)).resolves.toEqual([release])
    const url = new URL(fetcher.mock.calls[0][0])
    expect(url.origin).toBe('https://central.github.com')
    expect(url.searchParams.get('env')).toBe('test')
    expect(url.searchParams.get('limit')).toBe('2')
    expect(mocks.changelog).not.toHaveBeenCalled()
  })
})
