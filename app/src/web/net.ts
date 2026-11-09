// `net` for the webview build. A webview cannot open sockets; the address
// helpers are real implementations.
type Handler = (...args: any[]) => void

export type AddressInfo = { port: number; address: string; family: string }

class Emitter {
  private readonly handlers = new Map<string, Set<Handler>>()

  public on(event: string, handler: Handler) {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set())
    }
    this.handlers.get(event)!.add(handler)
    return this
  }

  public once(event: string, handler: Handler) {
    const wrapper: Handler = (...args) => {
      this.removeListener(event, wrapper)
      handler(...args)
    }
    return this.on(event, wrapper)
  }

  public removeListener(event: string, handler: Handler) {
    this.handlers.get(event)?.delete(handler)
    return this
  }

  public removeAllListeners(event?: string) {
    if (event === undefined) {
      this.handlers.clear()
    } else {
      this.handlers.delete(event)
    }
    return this
  }

  public emit(event: string, ...args: any[]) {
    const set = this.handlers.get(event)
    if (set === undefined || set.size === 0) {
      return false
    }
    set.forEach(handler => handler(...args))
    return true
  }
}

export class Socket extends Emitter {
  public write() {
    return false
  }
  public end() {
    return this
  }
  public destroy() {
    return this
  }
}

export class Server extends Emitter {
  /**
   * Never accepts a connection. The renderer starts a credential trampoline
   * server before every git command; in the webview that server cannot
   * exist, and the Rust shell strips the settings that would point git at
   * it (see `git_environment` in desktop-core/src/io.rs). Reporting
   * "listening" keeps that startup path working.
   */
  public listen(...args: any[]) {
    const callback = args[args.length - 1]
    queueMicrotask(() => {
      if (typeof callback === 'function') {
        callback()
      }
      this.emit('listening')
    })
    return this
  }
  public close(callback?: (error?: Error) => void) {
    callback?.()
    return this
  }
  public address(): AddressInfo | null {
    return { port: 0, address: '127.0.0.1', family: 'IPv4' }
  }
  public unref() {
    return this
  }
  public ref() {
    return this
  }
}

export function createServer(..._args: any[]) {
  return new Server()
}

export function connect(): Socket {
  throw new Error('net.connect is not available in the webview')
}

const v4Segment = '(?:25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)'
const v4 = new RegExp(`^${v4Segment}(?:\\.${v4Segment}){3}$`)

export function isIPv4(input: string) {
  return v4.test(input)
}

export function isIPv6(input: string) {
  if (!/^[0-9a-fA-F:.%\w]+$/.test(input) || !input.includes(':')) {
    return false
  }
  try {
    // The URL parser implements the full RFC 4291 grammar.
    new URL(`http://[${input.split('%')[0]}]/`)
    return true
  } catch {
    return false
  }
}

export function isIP(input: string) {
  return isIPv4(input) ? 4 : isIPv6(input) ? 6 : 0
}

export default { Server, Socket, createServer, connect, isIP, isIPv4, isIPv6 }
