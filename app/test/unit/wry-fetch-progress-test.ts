import { readFileSync } from 'node:fs'
import { JSDOM } from 'jsdom'
import { expect, it, vi } from 'vitest'

it('delivers native progress only to its pending invocation', async () => {
  const dom = new JSDOM('', { url: 'http://localhost/', runScripts: 'outside-only' })
  const window = dom.window as any
  const sent: Array<{ __rpc: number }> = []
  window.ipc = { postMessage: (body: string) => sent.push(JSON.parse(body)) }
  window.fetch = vi.fn()
  window.XMLHttpRequest = class { open() {} send() {} }
  window.eval(readFileSync('rust/crates/desktop-app/web/ipc-shim.js', 'utf8'))
  const first = vi.fn()
  const second = vi.fn()
  const broadcast = vi.fn()
  window.__desktop.on('fetch_output', broadcast)
  const firstRequest = window.__desktop.invokeWithProgress('desktop.backend.git_info', first, {})
  const secondRequest = window.__desktop.invokeWithProgress('desktop.backend.git_info', second, {})
  const firstId = sent[0].__rpc
  const secondId = sent[1].__rpc
  const event = { stream: 'stderr', data: 'dGVzdA==' }
  window.__desktop.progress(firstId, event)
  expect(first).toHaveBeenCalledOnce()
  expect(second).not.toHaveBeenCalled()
  expect(broadcast).not.toHaveBeenCalled()
  window.__desktop.resolve(firstId, { ok: true, value: null })
  await firstRequest
  window.__desktop.progress(firstId, event)
  expect(first).toHaveBeenCalledOnce()
  window.__desktop.progress(secondId, event)
  expect(second).toHaveBeenCalledOnce()
  window.__desktop.resolve(secondId, { ok: true, value: null })
  await secondRequest
  dom.window.close()
})
