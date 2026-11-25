import { afterEach, describe, expect, it, vi } from 'vitest'
import { readEmoji } from '../../src/lib/read-emoji'

describe('readEmoji', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('loads aliases from the bundled database in sorted order', async () => {
    const fetchMock = vi.fn(async () =>
      Response.json([
        { emoji: '😀', aliases: ['grinning'], description: 'Grinning face' },
        { aliases: ['shipit'], description: 'Custom squirrel' },
      ])
    )
    vi.stubGlobal('fetch', fetchMock)

    const emoji = await readEmoji()

    expect(fetchMock).toHaveBeenCalledOnce()
    expect([...emoji.keys()]).toEqual([':grinning:', ':shipit:'])
    expect(emoji.get(':shipit:')?.url).toContain('shipit.png')
    expect(emoji.get(':grinning:')?.url).toContain('unicode')
  })

  it('rejects when the bundled database cannot be read', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('missing', { status: 404 }))
    )

    await expect(readEmoji()).rejects.toThrow(
      'Unable to load bundled emoji database (404)'
    )
  })
})
