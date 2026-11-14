// @vitest-environment node
import { describe, it, vi } from 'vitest'
import assert from 'node:assert'
import { envForProxy } from '../../../src/lib/git/environment'
import { inheritHostProxy, resolveGitProxy } from '../../../src/lib/resolve-git-proxy'

vi.mock('../../../src/ui/main-process-proxy', () => ({
  resolveProxy: vi.fn(async () => 'DESKTOP_INHERIT_PROXY'),
}))

describe('git/environmnent', () => {
  const httpProxyUrl = 'http://proxy:8888/'
  const httpsProxyUrl = 'https://proxy:8888/'

  const nullResolver = () => Promise.resolve(undefined)
  const throwingResolver = () => Promise.reject(new Error('such error'))
  const defaultResolver = async (url: string) => {
    if (url.startsWith('http://')) {
      return httpProxyUrl
    } else if (url.startsWith('https://')) {
      return httpsProxyUrl
    } else {
      return undefined
    }
  }

  describe('envForProxy', () => {
    it('leaves a host-configured proxy in the child environment while signalling fallback', async () => {
      assert.equal(await resolveGitProxy('https://github.com/'), inheritHostProxy)
      assert.deepStrictEqual(
        await envForProxy('https://github.com/', {}, resolveGitProxy),
        {}
      )
    })
    it('sets the correct environment variable based on protocol', async () => {
      assert.deepStrictEqual(
        await envForProxy('https://github.com/', {}, defaultResolver),
        { https_proxy: httpsProxyUrl }
      )

      assert.deepStrictEqual(
        await envForProxy('http://github.com/', {}, defaultResolver),
        { http_proxy: httpProxyUrl }
      )
    })

    it('fails gracefully if resolver throws', async () => {
      assert.equal(
        await envForProxy('https://github.com/', {}, throwingResolver),
        undefined
      )
    })

    it("it doesn't set any variables if resolver returns undefined", async () => {
      assert.equal(
        await envForProxy('https://github.com/', {}, nullResolver),
        undefined
      )
    })

    it('sets the correct environment variable based on protocol', async () => {
      assert.deepStrictEqual(
        await envForProxy('https://github.com/', {}, defaultResolver),
        {
          https_proxy: httpsProxyUrl,
        }
      )

      assert.deepStrictEqual(
        await envForProxy('http://github.com/', {}, defaultResolver),
        {
          http_proxy: httpProxyUrl,
        }
      )
    })

    it('ignores unknown protocols', async () => {
      assert.equal(
        await envForProxy('ftp://github.com/', {}, defaultResolver),
        undefined
      )
    })

    it('does not override existing environment variables', async () => {
      assert.equal(
        await envForProxy(
          'https://github.com/',
          { https_proxy: 'foo' },
          defaultResolver
        ),
        undefined
      )

      assert.equal(
        await envForProxy(
          'https://github.com/',
          { HTTPS_PROXY: 'foo' },
          defaultResolver
        ),
        undefined
      )

      assert.equal(
        await envForProxy(
          'http://github.com/',
          { http_proxy: 'foo' },
          defaultResolver
        ),
        undefined
      )

      assert.equal(
        await envForProxy(
          'https://github.com/',
          { ALL_PROXY: 'foo' },
          defaultResolver
        ),
        undefined
      )

      assert.equal(
        await envForProxy(
          'https://github.com/',
          { all_proxy: 'foo' },
          defaultResolver
        ),
        undefined
      )
    })
  })
})
