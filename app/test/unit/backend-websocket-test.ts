import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

class FakeSocket {
  static readonly OPEN = 1
  static instances: FakeSocket[] = []
  readonly sent: Array<Record<string, unknown>> = []
  readyState = 0
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: ((event: { code: number }) => void) | null = null
  onerror: (() => void) | null = null

  constructor(readonly url: string) {
    FakeSocket.instances.push(this)
  }
  send(value: string): void {
    this.sent.push(JSON.parse(value))
  }
  open(): void {
    this.readyState = 1
    this.onopen?.()
  }
  message(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) })
  }
  disconnect(code = 1006): void {
    this.readyState = 3
    this.onclose?.({ code })
  }
  close(): void {
    this.disconnect(1000)
  }
}

const ready = {
  ready: true,
  version: 3,
  capabilities: { cancel: true, max_pending: 32 },
}
const clients: BackendClient[] = []

function client(): BackendClient {
  const value = new BackendClient()
  clients.push(value)
  return value
}

async function connected(): Promise<{
  client: BackendClient
  socket: FakeSocket
}> {
  const value = client()
  const pending = value.waitReady()
  const socket = FakeSocket.instances.at(-1)!
  socket.open()
  expect(socket.sent[0]).toEqual({ token: 'secret' })
  socket.message(ready)
  await pending
  return { client: value, socket }
}

describe('WebSocket backend transport', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    FakeSocket.instances = []
    sessionStorage.clear()
    Reflect.deleteProperty(window, '__desktop')
    window.history.replaceState(null, '', '/#token=secret')
    vi.stubGlobal('WebSocket', FakeSocket)
  })
  afterEach(() => {
    for (const value of clients.splice(0)) value.close()
    Reflect.deleteProperty(window, '__desktop')
    sessionStorage.clear()
    window.history.replaceState(null, '', '/')
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('retains the token for a reload while removing it from the URL', () => {
    const first = client()
    expect(first.transportKind).toBe('ws')
    expect(window.location.hash).toBe('')
    expect(sessionStorage.getItem('desktopBridgeToken')).toBe('secret')
    expect(client().transportKind).toBe('ws')
  })

  it('does not use a stored token when the URL explicitly provides an empty token', () => {
    sessionStorage.setItem('desktopBridgeToken', 'old-secret')
    window.history.replaceState(null, '', '/#token=')
    expect(client().transportKind).toBe('none')
    expect(sessionStorage.getItem('desktopBridgeToken')).toBeNull()
  })

  it('does not mistake the loopback shim for the native Wry transport', () => {
    const first = client()
    expect(first.transportKind).toBe('ws')
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { __transport: 'ws', invoke: vi.fn(), on: vi.fn() },
    })
    expect(client().transportKind).toBe('ws')
  })

  it('cleans up abort listeners after success and sends no late cancellation', async () => {
    const { client: value, socket } = await connected()
    const controller = new AbortController()
    const remove = vi.spyOn(controller.signal, 'removeEventListener')
    const pending = value.request(
      'snapshot',
      {},
      { signal: controller.signal, repositoryId: 'repo-1' }
    )
    expect(socket.sent[1]).toEqual({
      id: 1,
      method: 'snapshot',
      repository_id: 'repo-1',
    })
    socket.message({ id: 1, result: { root: 'repo-1' } })
    await expect(pending).resolves.toEqual({ root: 'repo-1' })
    expect(remove).toHaveBeenCalledWith('abort', expect.any(Function))
    controller.abort()
    expect(socket.sent).toHaveLength(2)
  })

  it('isolates fetch output to the active matching request', async () => {
    const { client: value, socket } = await connected()
    const first = vi.fn()
    const second = vi.fn()
    const broadcast = vi.fn()
    value.subscribe('fetch_output', broadcast)
    const firstResult = value.request('renderer_fetch', {}, { onProgress: first })
    const secondResult = value.request('renderer_fetch', {}, { onProgress: second })
    socket.message({ event: 'fetch_output', request_id: 1, stream: 'stderr', data: 'aGk=' })
    socket.message({ event: 'fetch_output', request_id: 2, stream: 'lfs', data: 'eA==' })
    socket.message({ event: 'fetch_output', request_id: 2, stream: 'invalid', data: 'eA==' })
    socket.message({ event: 'fetch_output', request_id: 999, stream: 'stderr', data: 'eA==' })
    expect(first).toHaveBeenCalledExactlyOnceWith({ stream: 'stderr', data: 'aGk=' })
    expect(second).toHaveBeenCalledExactlyOnceWith({ stream: 'lfs', data: 'eA==' })
    expect(broadcast).not.toHaveBeenCalled()
    socket.message({ id: 1, result: { code: 0 } })
    await firstResult
    socket.message({ event: 'fetch_output', request_id: 1, stream: 'stderr', data: 'bGF0ZQ==' })
    expect(first).toHaveBeenCalledOnce()
    socket.message({ id: 2, result: { code: 0 } })
    await secondResult
  })

  it('allows a fetch mutation to run beyond the ordinary request timeout', async () => {
    const { client: value, socket } = await connected()
    const pending = value.fetchRepositoryRemote('/repo', 'origin', '/source', false)
    socket.message({ id: 1, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[2]?.method).toBe('renderer_fetch_preview'))
    socket.message({ id: 2, result: { remote: 'origin', recurse_submodules: false, basis: 'a'.repeat(64) } })
    await vi.waitFor(() => expect(socket.sent[3]?.method).toBe('repository_binding'))
    socket.message({ id: 3, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[4]?.method).toBe('renderer_fetch'))
    await vi.advanceTimersByTimeAsync(170_001)
    expect(socket.sent.some(message => message.method === 'cancel')).toBe(false)
    socket.message({ id: 4, result: { code: 0, error_kind: null } })
    await expect(pending).resolves.toBeUndefined()
  })

  it.each([0, 1, 128])('accepts remote HEAD refresh exit code %s', async code => {
    const { client: value, socket } = await connected()
    const pending = value.refreshRepositoryRemoteHead('/repo', 'origin', '/source', false)
    socket.message({ id: 1, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[2]?.method).toBe('renderer_remote_head_preview'))
    socket.message({ id: 2, result: { remote: 'origin', basis: 'a'.repeat(64) } })
    await vi.waitFor(() => expect(socket.sent[3]?.method).toBe('repository_binding'))
    socket.message({ id: 3, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[4]?.method).toBe('renderer_remote_head'))
    socket.message({ id: 4, result: { code } })
    await expect(pending).resolves.toBeUndefined()
  })

  it('cancels a fetch mutation at the bounded deadline without retrying', async () => {
    const { client: value, socket } = await connected()
    const pending = value.fetchRepositoryRemote('/repo', 'origin', '/source', false)
    const result = expect(pending).rejects.toMatchObject({ code: 'timeout', retryable: false })
    socket.message({ id: 1, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[2]?.method).toBe('renderer_fetch_preview'))
    socket.message({ id: 2, result: { remote: 'origin', recurse_submodules: false, basis: 'a'.repeat(64) } })
    await vi.waitFor(() => expect(socket.sent[3]?.method).toBe('repository_binding'))
    socket.message({ id: 3, result: { root_id: 'repo-1' } })
    await vi.waitFor(() => expect(socket.sent[4]?.method).toBe('renderer_fetch'))
    await vi.advanceTimersByTimeAsync(3_600_001)
    await result
    expect(socket.sent.filter(message => message.method === 'renderer_fetch')).toHaveLength(1)
    expect(socket.sent).toContainEqual(expect.objectContaining({ method: 'cancel', request_id: 4 }))
  })

  it('allows a long Copilot turn to finish before the transport deadline', async () => {
    const { client: value, socket } = await connected()
    let finished = false
    const pending = value
      .copilotGenerateCommitMessage({ timeout_ms: 300_000 })
      .finally(() => {
        finished = true
      })
    await vi.advanceTimersByTimeAsync(170_001)
    expect(finished).toBe(false)
    expect(socket.sent).toHaveLength(2)
    socket.message({
      id: 1,
      result: {
        summary: 'fix: example',
        description: '',
        basis: 'staged_diff',
      },
    })
    await expect(pending).resolves.toMatchObject({ summary: 'fix: example' })
  })

  it('binds a Copilot request to the requested repository before sending it', async () => {
    const { client: value, socket } = await connected()
    const pending = value.copilotGenerateForRepository('/repo-a', {
      account: { host: 'github.com', login: 'Alice', id: 42 },
      paths: ['selected.txt'],
      remove_paths: ['old-name.txt'],
    })
    expect(socket.sent[1]).toEqual({
      id: 1,
      method: 'repository_binding',
      path: '/repo-a',
    })
    socket.message({ id: 1, result: { root_id: 'repo-a-id' } })
    for (let i = 0; i < 10 && socket.sent.length < 3; i++) {
      await Promise.resolve()
    }
    expect(socket.sent[2]).toEqual({
      id: 2,
      method: 'copilot_generate_commit_message',
      repository_id: 'repo-a-id',
      account: { host: 'github.com', login: 'Alice', id: 42 },
      paths: ['selected.txt'],
      remove_paths: ['old-name.txt'],
    })
    socket.message({
      id: 2,
      result: {
        summary: 'fix: example',
        description: '',
        basis: 'staged_diff',
      },
    })
    await expect(pending).resolves.toMatchObject({ summary: 'fix: example' })
  })

  it('sends only the selected identity when creating and closing a session', async () => {
    const { client: value, socket } = await connected()
    const creating = value.copilotCreateSession({
      account: { host: 'github.com', login: 'Alice', id: 42 },
    })
    expect(socket.sent[1]).toEqual({
      id: 1,
      method: 'copilot_create_session',
      account: { host: 'github.com', login: 'Alice', id: 42 },
    })
    socket.message({
      id: 1,
      result: { id: 'session-1', connection_id: 'connection', model: 'auto' },
    })
    await expect(creating).resolves.toMatchObject({ id: 'session-1' })
    const closing = value.copilotCloseSession('session-1')
    expect(socket.sent[2]).toEqual({
      id: 2,
      method: 'copilot_close_session',
      session_id: 'session-1',
    })
    socket.message({ id: 2, result: { closed: true } })
    await expect(closing).resolves.toEqual({ closed: true })
  })

  it('cancels a sent request on abort or timeout without replaying it', async () => {
    const { client: value, socket } = await connected()
    const controller = new AbortController()
    const pending = value.request(
      'commit',
      { summary: 'one' },
      { signal: controller.signal }
    )
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'cancelled',
      retryable: false,
    })
    controller.abort()
    await rejected
    expect(socket.sent[2]).toEqual({ id: 2, method: 'cancel', request_id: 1 })
    const timed = value.request('stage', {}, { deadlineMs: 20 })
    const timeout = expect(timed).rejects.toMatchObject({
      code: 'timeout',
      retryable: false,
    })
    await vi.advanceTimersByTimeAsync(20)
    await timeout
    expect(socket.sent[4]).toEqual({ id: 4, method: 'cancel', request_id: 3 })
  })

  it('does not start a request aborted or timed out during the handshake', async () => {
    const value = client()
    const controller = new AbortController()
    const pending = value.request('commit', {}, { signal: controller.signal })
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'cancelled',
    })
    controller.abort()
    await rejected
    const timed = value.request('stage', {}, { deadlineMs: 10 })
    const timeout = expect(timed).rejects.toMatchObject({ code: 'timeout' })
    await vi.advanceTimersByTimeAsync(10)
    await timeout
    const socket = FakeSocket.instances[0]
    socket.open()
    socket.message(ready)
    await value.waitReady()
    expect(socket.sent).toEqual([{ token: 'secret' }])
  })

  it('rejects in-flight work on disconnect and waits for the new handshake', async () => {
    const { client: value, socket } = await connected()
    const pending = value.request('commit', {}, {})
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'disconnected',
      retryable: false,
    })
    socket.disconnect()
    await rejected
    expect(value.isReady).toBe(false)
    const next = value.waitReady()
    await vi.advanceTimersByTimeAsync(1000)
    const replacement = FakeSocket.instances[1]
    replacement.open()
    socket.message(ready)
    expect(value.isReady).toBe(false)
    replacement.message(ready)
    await next
    expect(value.isReady).toBe(true)
    expect(replacement.sent).toEqual([{ token: 'secret' }])
  })

  it('rejects invalid authentication and does not reconnect', async () => {
    const value = client()
    const pending = value.waitReady()
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'unauthorized',
      retryable: false,
    })
    const socket = FakeSocket.instances[0]
    socket.open()
    socket.disconnect(1008)
    await rejected
    expect(sessionStorage.getItem('desktopBridgeToken')).toBeNull()
    await vi.advanceTimersByTimeAsync(5000)
    expect(FakeSocket.instances).toHaveLength(1)
    await expect(value.waitReady()).rejects.toMatchObject({
      code: 'unauthorized',
    })
  })

  it('rejects a failed handshake and permits a later reconnect', async () => {
    const value = client()
    const pending = value.waitReady()
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'disconnected',
    })
    FakeSocket.instances[0].disconnect()
    await rejected
    const next = value.waitReady()
    await vi.advanceTimersByTimeAsync(1000)
    const replacement = FakeSocket.instances[1]
    replacement.open()
    replacement.message(ready)
    await expect(next).resolves.toMatchObject({ ready: true })
  })

  it('bounds an unanswered handshake', async () => {
    const value = client()
    const pending = value.waitReady()
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'disconnected',
    })
    FakeSocket.instances[0].open()
    await vi.advanceTimersByTimeAsync(15_000)
    await rejected
  })

  it('rejects unsupported account scopes and malformed handshakes', async () => {
    const value = client()
    await expect(
      value.request('snapshot', {}, { accountId: 'other' })
    ).rejects.toMatchObject({ code: 'unsupported' })
    const pending = value.waitReady()
    const rejected = expect(pending).rejects.toMatchObject({ code: 'protocol' })
    const socket = FakeSocket.instances[0]
    socket.open()
    socket.message({
      ready: true,
      version: 4,
      capabilities: { cancel: true, max_pending: 32 },
    })
    await rejected
    expect(value.isReady).toBe(false)
  })

  it('rejects in-flight work on malformed frames without reconnecting', async () => {
    const { client: value, socket } = await connected()
    const pending = value.request('commit', {}, {})
    const rejected = expect(pending).rejects.toMatchObject({
      code: 'protocol',
      retryable: false,
    })
    socket.onmessage?.({ data: '{' })
    await rejected
    await vi.advanceTimersByTimeAsync(5000)
    expect(FakeSocket.instances).toHaveLength(1)
  })

  it('delivers events to subscribers and stops after unsubscribe', async () => {
    const { client: value, socket } = await connected()
    const listener = vi.fn()
    const unsubscribe = value.subscribe('repository_changed', listener)
    socket.message({
      event: 'repository_changed',
      snapshot: { root_id: 'one' },
    })
    socket.message({ event: 'repository_changed', snapshot: null })
    unsubscribe()
    socket.message({
      event: 'repository_changed',
      snapshot: { root_id: 'two' },
    })
    expect(listener).toHaveBeenCalledTimes(2)
    expect(listener).toHaveBeenNthCalledWith(1, { root_id: 'one' })
    expect(listener).toHaveBeenNthCalledWith(2, null)
  })
})
