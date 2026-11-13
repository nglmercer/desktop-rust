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
  window.__desktop.on('commit_output', broadcast)
  const firstRequest = window.__desktop.invokeWithProgress('desktop.backend.git_info', first, {})
  const secondRequest = window.__desktop.invokeWithProgress('desktop.backend.git_info', second, {})
  const firstId = sent[0].__rpc
  const secondId = sent[1].__rpc
  const event = { stream: 'stderr', data: 'dGVzdA==' }
  window.__desktop.progress(firstId, event)
  expect(first).toHaveBeenCalledOnce()
  expect(second).not.toHaveBeenCalled()
  expect(broadcast).not.toHaveBeenCalled()
  const commit = { event: 'commit_output', checkpoint_id: 'checkpoint', stream: 'stderr', data: 'dGVzdA==' }
  window.__desktop.progress(secondId, commit)
  expect(second).toHaveBeenCalledWith(commit)
  expect(broadcast).not.toHaveBeenCalled()
  window.__desktop.resolve(firstId, { ok: true, value: null })
  await firstRequest
  window.__desktop.progress(firstId, event)
  expect(first).toHaveBeenCalledOnce()
  window.__desktop.progress(secondId, event)
  expect(second).toHaveBeenCalledTimes(2)
  window.__desktop.resolve(secondId, { ok: true, value: null })
  await secondRequest
  dom.window.close()
})

it('allows only a bounded fetch mutation to extend the IPC timeout', async () => {
  const dom = new JSDOM('', { url: 'http://localhost/', runScripts: 'outside-only' })
  const window = dom.window as any
  const sent: Array<{ __rpc: number }> = []
  window.ipc = { postMessage: (body: string) => sent.push(JSON.parse(body)) }
  window.fetch = vi.fn()
  window.XMLHttpRequest = class { open() {} send() {} }
  const timers = vi.spyOn(window, 'setTimeout')
  window.eval(readFileSync('rust/crates/desktop-app/web/ipc-shim.js', 'utf8'))
  const mutation = { operation: { type: 'fetch_remote', operation: { method: 'renderer_fetch' } }, deadline_ms: 3_600_000 }
  const head = { operation: { type: 'fetch_remote', operation: { method: 'renderer_remote_head' } }, deadline_ms: 3_600_000 }
  const preview = { operation: { type: 'fetch_remote', operation: { method: 'renderer_fetch_preview' } }, deadline_ms: 3_600_000 }
  const calls = [
    window.__desktop.invoke('desktop.backend.git_info', mutation),
    window.__desktop.invoke('desktop.backend.git_info', head),
    window.__desktop.invoke('desktop.backend.git_info', preview),
    window.__desktop.invoke('desktop.backend.git_info', { ...mutation, deadline_ms: 3_600_001 }),
  ]
  expect(timers.mock.calls.filter(call => call[1] === 3_630_000)).toHaveLength(2)
  expect(timers.mock.calls.filter(call => call[1] === 180_000)).toHaveLength(2)
  sent.forEach(message => window.__desktop.resolve(message.__rpc, { ok: true, value: null }))
  await Promise.all(calls)
  dom.window.close()
})

it('refuses request-owned progress before attempting the HTTP fallback', async () => {
  const dom = new JSDOM('', { url: 'http://localhost/', runScripts: 'outside-only' })
  const window = dom.window as any
  const nativeFetch = vi.fn()
  window.fetch = nativeFetch
  window.XMLHttpRequest = class { open() {} send() {} }
  window.eval(readFileSync('rust/crates/desktop-app/web/ipc-shim.js', 'utf8'))
  await expect(window.__desktop.invokeWithProgress('desktop.backend.git_info', vi.fn(), {})).rejects.toMatchObject({ code: 'unsupported' })
  expect(nativeFetch).not.toHaveBeenCalled()
  dom.window.close()
})
