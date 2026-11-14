import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({ transport: 'ws', write: vi.fn() }))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    writeRepositoryCreationFile: mocks.write,
  }),
}))

describe('bundled repository templates', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  afterEach(() => vi.unstubAllGlobals())

  it.each(['ws', 'wry'])(
    'loads license and gitignore data over the %s origin',
    async transport => {
      mocks.transport = transport
      const requests: string[] = []
      vi.stubGlobal(
        'fetch',
        vi.fn(async (input: URL | string) => {
          const url = new URL(String(input))
          requests.push(`${url.pathname}${url.search}`)
          if (url.pathname === '/__bundle') {
            expect(url.searchParams.get('path')).toBe('static/gitignore')
            return new Response(
              JSON.stringify({
                kind: 'directory',
                entries: [
                  { name: 'Node.gitignore', isDirectory: false },
                  { name: 'C++.gitignore', isDirectory: false },
                  { name: 'README.md', isDirectory: false },
                ],
              })
            )
          }
          if (url.pathname === '/static/available-licenses.json') {
            return new Response(
              JSON.stringify([
                {
                  name: 'Example',
                  featured: true,
                  hidden: false,
                  body: 'Example license\n',
                },
              ])
            )
          }
          if (url.pathname === '/static/gitignore/C%2B%2B.gitignore') {
            return new Response('*.o\n')
          }
          return new Response('', { status: 404 })
        })
      )
      const { getGitIgnoreNames, writeGitIgnore } = await import(
        '../../src/ui/add-repository/gitignores'
      )
      const { getLicenses } = await import(
        '../../src/ui/add-repository/licenses'
      )
      expect(await getGitIgnoreNames()).toEqual(['Node', 'C++'])
      expect(await getLicenses()).toEqual([
        {
          name: 'Example',
          featured: true,
          hidden: false,
          body: 'Example license\n',
        },
      ])
      await writeGitIgnore('/repo', 'C++')
      expect(mocks.write).toHaveBeenCalledWith('/repo', 'gitignore', '*.o\n')
      await expect(writeGitIgnore('/repo', '../outside')).rejects.toThrow(
        'Unknown gitignore'
      )
      expect(requests).toEqual([
        '/__bundle?path=static%2Fgitignore',
        '/static/available-licenses.json',
        '/static/gitignore/C%2B%2B.gitignore',
      ])
    }
  )

  it('rejects an unsafe directory entry without fetching a template', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({
              kind: 'directory',
              entries: [{ name: '../outside.gitignore' }],
            })
          )
      )
    )
    const { getGitIgnoreNames } = await import(
      '../../src/ui/add-repository/gitignores'
    )
    await expect(getGitIgnoreNames()).rejects.toThrow(
      'Invalid bundled gitignore template name'
    )
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('reports missing bundle data and retries after the failure', async () => {
    const fetcher = vi
      .fn()
      .mockResolvedValueOnce(new Response('', { status: 503 }))
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ kind: 'directory', entries: [] }))
      )
    vi.stubGlobal('fetch', fetcher)
    const { getGitIgnoreNames } = await import(
      '../../src/ui/add-repository/gitignores'
    )
    await expect(getGitIgnoreNames()).rejects.toThrow('503')
    await expect(getGitIgnoreNames()).resolves.toEqual([])
  })
})
