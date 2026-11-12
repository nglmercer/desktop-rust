/** Local WebSocket bridge for saved-account integration tests. */
import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import { createRequire } from 'node:module'
import { createInterface } from 'node:readline'
import {
  type Accounts,
  type AccountRepo,
  BINARY,
  clean,
  object,
  type Json,
} from './accounts'

interface IFixtureSocket {
  onmessage: ((event: { data: string }) => void) | null
  onopen: (() => void) | null
  onerror: (() => void) | null
  send(data: string): void
  close(): void
}
interface IFixtureDom {
  readonly window: {
    readonly WebSocket: new (url: string) => IFixtureSocket
    close(): void
  }
}
const JSDOM = createRequire(import.meta.url)('jsdom').JSDOM as new (
  html: string,
  options: { url: string }
) => IFixtureDom

async function bounded<T>(promise: Promise<T>, timeout = 12_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Account bridge timed out')),
          timeout
        )
      }),
    ])
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
}

export class AccountBridge {
  private nextId = 100
  private readonly messages: Json[] = []
  private waiting: ((value: Json) => void) | undefined
  private constructor(
    private readonly child: ChildProcess,
    private readonly dom: IFixtureDom,
    private readonly socket: IFixtureSocket
  ) {
    socket.onmessage = event => {
      const message = object(JSON.parse(event.data))
      clean(message)
      if (this.waiting !== undefined) {
        const resolve = this.waiting
        this.waiting = undefined
        resolve(message)
      } else this.messages.push(message)
    }
  }
  static async start(
    suite: Accounts,
    repo: AccountRepo
  ): Promise<AccountBridge> {
    const child = spawn(
      BINARY,
      ['--config', suite.registry, '-C', repo.root, 'serve', '--port', '0'],
      { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] }
    )
    let dom: IFixtureDom | undefined
    try {
      assert.ok(child.stderr)
      const lines = createInterface({ input: child.stderr })
      const line = await bounded(
        new Promise<string>((resolve, reject) => {
          lines.once('line', resolve)
          child.once('error', reject)
          child.once('exit', code =>
            reject(new Error(`Account bridge exited before startup: ${code}`))
          )
        })
      )
      const match = /^Open (http:\/\/127\.0\.0\.1:\d+)\/#token=(\S+)$/.exec(
        line
      )
      assert.ok(match, 'Invalid bridge startup announcement')
      const origin = match[1]
      const token = match[2]
      dom = new JSDOM('', { url: origin })
      const socket = new dom.window.WebSocket(
        `${origin.replace('http:', 'ws:')}/ws`
      )
      const bridge = new AccountBridge(child, dom, socket)
      await bounded(
        new Promise<void>((resolve, reject) => {
          socket.onopen = resolve
          socket.onerror = () =>
            reject(new Error('Account bridge WebSocket failed'))
        })
      )
      socket.send(JSON.stringify({ token }))
      const ready = await bridge.receive()
      assert.equal(ready.ready, true)
      return bridge
    } catch (error) {
      dom?.window.close()
      if (child.exitCode === null && child.signalCode === null)
        child.kill('SIGKILL')
      throw error
    }
  }
  async receive(): Promise<Json> {
    while (true) {
      const message =
        this.messages.shift() ??
        (await bounded(
          new Promise<Json>(resolve => {
            this.waiting = resolve
          })
        ))
      if (message.event === undefined) return message
    }
  }
  send(id: number, method: string, params: Json = {}): void {
    this.socket.send(JSON.stringify({ id, method, ...params }))
  }
  async call(method: string, params: Json = {}, fail = false): Promise<Json> {
    const id = ++this.nextId
    this.send(id, method, params)
    const reply = await this.receive()
    assert.equal(reply.id, id)
    if (fail) {
      assert.ok(reply.error)
      return reply
    }
    assert.ok(reply.result !== undefined)
    return object(reply.result)
  }
  async close(): Promise<void> {
    this.socket.close()
    this.dom.window.close()
    if (this.child.exitCode === null && this.child.signalCode === null) {
      const stopped = new Promise<void>(resolve =>
        this.child.once('exit', () => resolve())
      )
      this.child.kill('SIGINT')
      try {
        await bounded(stopped, 8_000)
      } catch {
        this.child.kill('SIGKILL')
        await bounded(stopped, 5_000)
      }
    }
  }
}
