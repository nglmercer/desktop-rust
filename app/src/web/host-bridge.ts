/**
 * The renderer's window onto the desktop host: IPC, shell helpers, dialogs and
 * app metadata, all served through the Rust bridge.
 * Rust bridge is injected as window.__desktop (see desktop-app/web/ipc-shim.ts).
 * For Termux loopback, window.desktop is bridge over WebSocket.
 */

type Listener = (event: any, ...args: any[]) => void

class HostIpc {
  private listeners = new Map<string, Set<Listener>>()

  private bridge() {
    return (
      (window as any).__desktop ||
      (window as any).desktop ||
      (window as any).ipcRenderer
    )
  }

  invoke(channel: string, ...args: any[]) {
    const b = this.bridge()
    if (b?.invoke) return b.invoke(channel, ...args)
    return Promise.reject(
      new Error(`[vite-shim] no bridge for invoke(${channel})`)
    )
  }

  send(channel: string, ...args: any[]) {
    const b = this.bridge()
    if (b?.send) return b.send(channel, ...args)
    // fallback
    console.warn('[shim] send no bridge', channel)
  }

  sendSync(channel: string, ...args: any[]) {
    // No sync IPC in webview — mirror wry behavior: warn and return promise
    console.warn(`[shim] sendSync(${channel}) is async under wry`)
    return this.invoke(channel, ...args)
  }

  on(channel: string, listener: Listener) {
    if (!this.listeners.has(channel)) this.listeners.set(channel, new Set())
    this.listeners.get(channel)!.add(listener)
    const b = this.bridge()
    if (b?.on) b.on(channel, listener)
    return this
  }

  once(channel: string, listener: Listener) {
    const wrap: Listener = (event, ...args) => {
      this.removeListener(channel, wrap)
      listener(event, ...args)
    }
    return this.on(channel, wrap)
  }

  removeListener(channel: string, listener: Listener) {
    this.listeners.get(channel)?.delete(listener)
    const b = this.bridge()
    if (b?.removeListener) b.removeListener(channel, listener)
    return this
  }

  removeAllListeners(channel?: string) {
    if (channel) this.listeners.delete(channel)
    else this.listeners.clear()
    const b = this.bridge()
    if (b?.removeAllListeners) b.removeAllListeners(channel)
    return this
  }

  addListener(channel: string, listener: Listener) {
    return this.on(channel, listener)
  }

  _dispatch(channel: string, ...args: any[]) {
    const set = this.listeners.get(channel)
    if (set)
      set.forEach(fn => {
        try {
          fn({ sender: null }, ...args)
        } catch (e) {
          console.error(e)
        }
      })
  }
}

export const hostIpc = new HostIpc()

/** Shell integration the webview cannot do itself. */
export const hostShell = {
  /** Windows shortcut metadata is not readable from the webview. */
  readShortcutLink: (_p: string): { toastActivatorClsid?: string } => ({}),
}

/** Native path lookup for dropped files; the webview exposes none. */
export const hostFiles = {
  getPathForFile: (_file: File) => '',
}

export interface HostApp {
  getVersion(): string
  getName(): string
  getPath(name: string): string
  getAppPath(): string
  runningUnderARM64Translation?: boolean
  on(event: string, listener: (...args: any[]) => void): void
  once(event: string, listener: (...args: any[]) => void): void
}

export const hostApp: HostApp = {
  getVersion: () => __APP_VERSION__,
  getName: () => __APP_NAME__,
  getPath: (_name: string) => '',
  getAppPath: () => '',
  on: (_event: string, _listener: (...args: any[]) => void) => {},
  once: (_event: string, _listener: (...args: any[]) => void) => {},
}
