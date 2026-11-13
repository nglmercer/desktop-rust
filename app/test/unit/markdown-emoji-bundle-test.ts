import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { JSDOM } from 'jsdom'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Emoji } from '../../src/lib/emoji'
import { EmojiFilter } from '../../src/lib/markdown-filters/emoji-filter'
import { bundleUrl } from '../../src/web/bundle'

const emoji = (url: string): Emoji => ({ aliases: ['shipit'], url })
const makeFilter = (url: string) =>
  new EmojiFilter(new Map([[':shipit:', emoji(url)]]))
const input = () => document.createTextNode('hello :shipit:!')

afterEach(() => {
  vi.unstubAllGlobals()
  document.head.querySelector('base')?.remove()
})

describe('bundled emoji in sandboxed Markdown', () => {
  it('loads and caches the custom emoji from the Rust renderer bundle', async () => {
    const base = document.createElement('base')
    base.href = 'http://localhost/session/index.html'
    document.head.appendChild(base)
    const fetchImage = vi.fn().mockResolvedValue({
      ok: true,
      arrayBuffer: async () => Uint8Array.from([137, 80, 78, 71]).buffer,
    })
    vi.stubGlobal('fetch', fetchImage)
    const assetUrl = bundleUrl('/__desktop_app__/emoji/shipit.png')
    expect(assetUrl).toBe('http://localhost/session/emoji/shipit.png')
    if (assetUrl === null) throw new Error('Expected bundled emoji URL')
    const filter = makeFilter(assetUrl)
    for (let i = 0; i < 2; i++) {
      const nodes = await filter.filter(input())
      expect(nodes).not.toBeNull()
      expect(nodes?.[1]).toBeInstanceOf(HTMLImageElement)
      expect((nodes?.[1] as HTMLImageElement).src).toBe(
        'data:image/png;base64,iVBORw=='
      )
    }
    expect(fetchImage).toHaveBeenCalledOnce()
    expect(fetchImage).toHaveBeenCalledWith(
      'http://localhost/session/emoji/shipit.png',
      { redirect: 'error' }
    )
  })

  it('loads a bundled emoji using the Wry custom protocol', async () => {
    const base = document.createElement('base')
    base.href = 'desktop://localhost/index.html'
    document.head.appendChild(base)
    const fetchImage = vi.fn().mockResolvedValue({
      ok: true,
      arrayBuffer: async () => Uint8Array.from([1, 2, 3]).buffer,
    })
    vi.stubGlobal('fetch', fetchImage)
    const nodes = await makeFilter(
      'desktop://localhost/emoji/shipit.png'
    ).filter(input())
    expect((nodes?.[1] as HTMLImageElement).src).toBe(
      'data:image/png;base64,AQID'
    )
    expect(bundleUrl('/__desktop_app__/emoji/shipit.png')).toBe(
      'desktop://localhost/emoji/shipit.png'
    )
    expect(fetchImage).toHaveBeenCalledWith(
      'desktop://localhost/emoji/shipit.png',
      { redirect: 'error' }
    )
  })

  it.each([
    'https://example.com/emoji/shipit.png',
    'http://localhost/emoji/../secret.png',
    'http://localhost/emoji/%2fsecret.png',
    'http://localhost/emoji/shipit.png?other=1',
  ])('does not fetch untrusted emoji URL %s', async url => {
    const fetchImage = vi.fn()
    vi.stubGlobal('fetch', fetchImage)
    await makeFilter(url).filter(input())
    expect(fetchImage).not.toHaveBeenCalled()
  })

  it('reads a real bundle response and refuses redirects', async () => {
    const requested: string[] = []
    const server = createServer((request, response) => {
      requested.push(request.url ?? '')
      if (request.url === '/session/emoji/redirect.png') {
        response.writeHead(302, { location: '/secret' }).end()
      } else {
        response
          .writeHead(200, { 'content-type': 'image/png' })
          .end(Buffer.from([1, 2, 3]))
      }
    })
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
    try {
      const address = server.address()
      if (address === null || typeof address === 'string')
        throw new Error('Missing server address')
      const base = document.createElement('base')
      base.href = `http://127.0.0.1:${address.port}/session/index.html`
      document.head.appendChild(base)
      const assetUrl = bundleUrl('/__desktop_app__/emoji/shipit.png')
      if (assetUrl === null) throw new Error('Expected bundled emoji URL')
      const nodes = await makeFilter(assetUrl).filter(input())
      expect((nodes?.[1] as HTMLImageElement).src).toBe(
        'data:image/png;base64,AQID'
      )
      await makeFilter(
        new URL('emoji/redirect.png', base.href).toString()
      ).filter(input())
      expect(requested).toEqual([
        '/session/emoji/shipit.png',
        '/session/emoji/redirect.png',
      ])
    } finally {
      await new Promise<void>((resolve, reject) =>
        server.close(error => (error ? reject(error) : resolve()))
      )
    }
  })

  it('keeps Electron file URL loading', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'desktop-emoji-filter-'))
    const dom = new JSDOM('', { url: 'file:///app/index.html' })
    try {
      vi.stubGlobal('document', dom.window.document)
      const file = join(directory, 'shipit.png')
      await writeFile(file, Uint8Array.from([1, 2, 3]))
      const nodes = await makeFilter(pathToFileURL(file).toString()).filter(
        input()
      )
      expect((nodes?.[1] as HTMLImageElement).src).toBe(
        'data:image/png;base64,AQID'
      )
    } finally {
      dom.window.close()
      await rm(directory, { recursive: true, force: true })
    }
  })

  it.each(['http://localhost/index.html', 'desktop://localhost/index.html'])(
    'refuses file emoji URLs in the Rust renderer at %s',
    async page => {
      const directory = await mkdtemp(join(tmpdir(), 'desktop-emoji-secret-'))
      const dom = new JSDOM('', { url: page })
      try {
        vi.stubGlobal('document', dom.window.document)
        const secret = join(directory, 'secret.png')
        await writeFile(secret, Uint8Array.from([1, 2, 3]))
        const nodes = await makeFilter(pathToFileURL(secret).toString()).filter(
          input()
        )
        expect(nodes?.some(node => node.nodeName === 'IMG')).toBe(false)
      } finally {
        dom.window.close()
        await rm(directory, { recursive: true, force: true })
      }
    }
  )
})
