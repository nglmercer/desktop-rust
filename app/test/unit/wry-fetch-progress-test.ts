import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import { afterAll, beforeAll, expect, it, vi } from 'vitest'

let output: string
let bootstrap: string
beforeAll(() => {
  output = mkdtempSync(join(tmpdir(), 'desktop-ipc-shim-'))
  execFileSync(
    process.execPath,
    [
      resolve('node_modules/vite/bin/vite.js'),
      'build',
      '-c',
      'vite.ipc-shim.config.ts',
      '--outDir',
      output,
      '--emptyOutDir',
    ],
    { cwd: process.cwd(), stdio: 'pipe' }
  )
  bootstrap = readFileSync(join(output, 'ipc-shim.js'), 'utf8')
})
afterAll(() => rmSync(output, { recursive: true, force: true }))

it('delivers native progress only to its pending invocation', async () => {
  const dom = new JSDOM('', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  const sent: Array<{ __rpc: number }> = []
  window.ipc = { postMessage: (body: string) => sent.push(JSON.parse(body)) }
  window.fetch = vi.fn()
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  window.eval(bootstrap)
  const first = vi.fn()
  const second = vi.fn()
  const broadcast = vi.fn()
  window.__desktop.on('fetch_output', broadcast)
  window.__desktop.on('commit_output', broadcast)
  const firstRequest = window.__desktop.invokeWithProgress(
    'desktop.backend.git_info',
    first,
    {}
  )
  const secondRequest = window.__desktop.invokeWithProgress(
    'desktop.backend.git_info',
    second,
    {}
  )
  const firstId = sent[0].__rpc
  const secondId = sent[1].__rpc
  const event = { stream: 'stderr', data: 'dGVzdA==' }
  window.__desktop.progress(firstId, event)
  expect(first).toHaveBeenCalledOnce()
  expect(second).not.toHaveBeenCalled()
  expect(broadcast).not.toHaveBeenCalled()
  const commit = {
    event: 'commit_output',
    checkpoint_id: 'checkpoint',
    stream: 'stderr',
    data: 'dGVzdA==',
  }
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

it('allows only bounded fetch and push mutations to extend the IPC timeout', async () => {
  const dom = new JSDOM('', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  const sent: Array<{ __rpc: number }> = []
  window.ipc = { postMessage: (body: string) => sent.push(JSON.parse(body)) }
  window.fetch = vi.fn()
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  const timers = vi.spyOn(window, 'setTimeout')
  window.eval(bootstrap)
  const mutation = {
    operation: {
      type: 'fetch_remote',
      operation: { method: 'renderer_fetch' },
    },
    deadline_ms: 3_600_000,
  }
  const head = {
    operation: {
      type: 'fetch_remote',
      operation: { method: 'renderer_remote_head' },
    },
    deadline_ms: 3_600_000,
  }
  const preview = {
    operation: {
      type: 'fetch_remote',
      operation: { method: 'renderer_fetch_preview' },
    },
    deadline_ms: 3_600_000,
  }
  const push = {
    operation: {
      type: 'fetch_remote',
      operation: { method: 'renderer_push' },
    },
    deadline_ms: 3_600_000,
  }
  const calls = [
    window.__desktop.invoke('desktop.backend.git_info', mutation),
    window.__desktop.invoke('desktop.backend.git_info', head),
    window.__desktop.invoke('desktop.backend.git_info', push),
    window.__desktop.invoke('desktop.backend.git_info', preview),
    window.__desktop.invoke('desktop.backend.git_info', {
      ...mutation,
      deadline_ms: 3_600_001,
    }),
  ]
  expect(timers.mock.calls.filter(call => call[1] === 3_630_000)).toHaveLength(
    3
  )
  expect(timers.mock.calls.filter(call => call[1] === 180_000)).toHaveLength(2)
  sent.forEach(message =>
    window.__desktop.resolve(message.__rpc, { ok: true, value: null })
  )
  await Promise.all(calls)
  dom.window.close()
})

it('refuses request-owned progress before attempting the HTTP fallback', async () => {
  const dom = new JSDOM('', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  const nativeFetch = vi.fn()
  window.fetch = nativeFetch
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  window.eval(bootstrap)
  await expect(
    window.__desktop.invokeWithProgress('desktop.backend.git_info', vi.fn(), {})
  ).rejects.toMatchObject({ code: 'unsupported' })
  expect(nativeFetch).not.toHaveBeenCalled()
  dom.window.close()
})

it('buffers events until the renderer subscribes and becomes ready', () => {
  const dom = new JSDOM('', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  window.fetch = vi.fn()
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  window.eval(bootstrap)
  const listener = vi.fn()
  window.__desktop.emit('state-changed', [{ selected: 7 }])
  expect(window.__desktop.state).toEqual({ selected: 7 })
  window.__desktop.on('state-changed', listener)
  window.__desktop.ready()
  expect(listener).toHaveBeenCalledWith({ sender: null }, { selected: 7 })
  window.__desktop.removeListener('state-changed', listener)
  window.__desktop.emit('state-changed', [{ selected: 8 }])
  expect(listener).toHaveBeenCalledTimes(1)
  dom.window.close()
})

it('returns the selected nested context-menu index path', async () => {
  const dom = new JSDOM('<html><head></head><body></body></html>', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  window.fetch = vi.fn()
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  window.eval(bootstrap)
  const choice = window.__desktop.invoke('show-contextual-menu', [
    {
      label: 'Group',
      submenu: [{ label: 'Disabled', enabled: false }, { label: 'Choose' }],
    },
  ])
  const rows = Array.from(
    window.document.querySelectorAll('.gd-row')
  ) as HTMLElement[]
  const disabled = rows.find(row => row.textContent === 'Disabled')
  const selected = rows.find(row => row.textContent === 'Choose')
  expect(disabled?.classList.contains('disabled')).toBe(true)
  selected?.click()
  expect(await choice).toEqual([0, 1])
  expect(window.document.querySelector('.gd-ctx')).toBeNull()
  dom.window.close()
})

it('proxies cross-origin fetch through native IPC with response bytes intact', async () => {
  const dom = new JSDOM('', {
    url: 'http://localhost/',
    runScripts: 'outside-only',
  })
  const window = dom.window as any
  const sent: Array<{ __rpc: number; channel: string; args: any[] }> = []
  window.ipc = { postMessage: (body: string) => sent.push(JSON.parse(body)) }
  window.fetch = vi.fn()
  window.Request = Request
  window.Response = Response
  window.Headers = Headers
  window.XMLHttpRequest = class {
    open() {}
    send() {}
  }
  window.eval(bootstrap)
  const response = window.fetch('https://example.test/resource', {
    method: 'POST',
    body: 'hello',
  })
  await vi.waitFor(() => expect(sent).toHaveLength(1))
  expect(sent[0].channel).toBe('desktop.io.http-fetch')
  expect(sent[0].args[0]).toMatchObject({
    url: 'https://example.test/resource',
    method: 'POST',
    body: 'aGVsbG8=',
  })
  window.__desktop.resolve(sent[0].__rpc, {
    ok: true,
    value: {
      status: 200,
      statusText: 'OK',
      headers: { 'content-type': 'text/plain' },
      body: 'd29ybGQ=',
      url: 'https://example.test/resource',
    },
  })
  expect(await (await response).text()).toBe('world')
  dom.window.close()
})
