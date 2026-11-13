/**
 * Injected before the UI entry. Mirrors `rust/crates/desktop-app/web/ipc-
 * shim.js` but for the Vite dev / loopback-bridge path. In the production
 * Tao/Wry host, `ipc-shim.js` is already injected via the wry
 * initialization script and `window.__desktop` exists — this loader is a
 * no-op there. Otherwise it installs a minimal `__desktop` marker and,
 * when the page was served by the Rust bridge (`desktop-rs serve`, Termux
 * or the loopback server), attaches the typed WebSocket transport. Legacy
 * `desktop.io.*` and synchronous shell commands are not supported by it.
 *
 * Typed requests wait for authenticated WebSocket readiness. The marker
 * remains distinct from the real Wry bridge for subsequent client instances.
 */
import { backend } from '../backend/client'
import { BundleRoot } from './bundle'

if (typeof window !== 'undefined' && !Reflect.get(window, '__desktop')) {
  const client = backend()
  const subscriptions = new Map<
    string,
    Map<(...args: unknown[]) => void, () => void>
  >()

  const shim = {
    __transport: client.transportKind,
    invoke: async (channel: string, ...args: unknown[]) => {
      if (client.transportKind === 'none') {
        throw new Error(
          `[vite] bridge not attached — run via desktop-rs serve or the Tao/Wry shell`
        )
      }
      if (client.transportKind === 'ws') {
        // Browsers own their window and zoom state; these Electron queries
        // must not be sent as repository RPCs to the Rust bridge.
        switch (channel) {
          case 'get-current-window-state':
          case 'get-current-window-zoom-factor':
            return undefined
          case 'get-app-path':
            return BundleRoot
          case 'is-window-focused':
            return document.hasFocus()
          case 'should-use-dark-colors':
            return (
              window.matchMedia?.('(prefers-color-scheme: dark)').matches ??
              false
            )
          case 'write-clipboard-text': {
            const [text] = args
            if (typeof text !== 'string') {
              throw new TypeError('Clipboard text must be a string')
            }
            if (!navigator.clipboard?.writeText) {
              throw new Error('Clipboard access is unavailable')
            }
            return navigator.clipboard.writeText(text)
          }
          case 'open-external': {
            const [value] = args
            if (typeof value !== 'string') {
              return false
            }
            try {
              const url = new URL(value)
              if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) {
                return false
              }
              window.open(url.href, '_blank', 'noopener,noreferrer')
              return true
            } catch {
              return false
            }
          }
        }
      }
      // Only typed methods are implemented by the WebSocket bridge.
      const method = channel.startsWith('desktop.backend.')
        ? channel.slice('desktop.backend.'.length)
        : channel
      const [params] = args
      return client.request(method, params ?? {})
    },
    send: (channel: string, ...args: unknown[]) => {
      if (client.transportKind === 'ws') {
        if (channel === 'select-all-window-contents') {
          const active = document.activeElement
          if (
            active instanceof HTMLInputElement ||
            active instanceof HTMLTextAreaElement
          ) {
            active.select()
          } else {
            const target =
              active instanceof HTMLElement && active.isContentEditable
                ? active
                : document.body
            if (target) {
              window.getSelection()?.selectAllChildren(target)
            }
          }
          return
        }
        // Simplex over WS: fire-and-forget snapshot/preferences pushes are
        // server-driven; client sends are RPCs, so a stray `send` is a no-op.
        return
      }
      Reflect.get(window, 'ipc')?.postMessage?.(
        JSON.stringify({ channel, args })
      )
    },
    on: (channel: string, listener: (...args: unknown[]) => void) => {
      if (client.transportKind === 'ws') {
        let listeners = subscriptions.get(channel)
        if (!listeners) {
          listeners = new Map()
          subscriptions.set(channel, listeners)
        }
        if (!listeners.has(listener)) {
          listeners.set(
            listener,
            client.subscribe(channel, (...args) =>
              listener({ sender: null }, ...args)
            )
          )
        }
      }
      return Reflect.get(window, '__desktop')
    },
    removeListener: (
      channel: string,
      listener: (...args: unknown[]) => void
    ) => {
      const listeners = subscriptions.get(channel)
      listeners?.get(listener)?.()
      listeners?.delete(listener)
      if (listeners?.size === 0) {
        subscriptions.delete(channel)
      }
      return Reflect.get(window, '__desktop')
    },
    removeAllListeners: (channel?: string) => {
      for (const [event, listeners] of subscriptions) {
        if (channel !== undefined && channel !== event) {
          continue
        }
        for (const unsubscribe of listeners.values()) {
          unsubscribe()
        }
        subscriptions.delete(event)
      }
      return Reflect.get(window, '__desktop')
    },
    emit: () => {},
    resolve: () => {},
    // Exposed for `pendingLimitFromHealth` in `app/src/backend/client.ts`.
    pendingPromises: client.capabilities().max_pending,
  }
  Reflect.set(window, '__desktop', shim)

  // Prefer the WebSocket bridge when we are not in the Wry shell.
  if (client.transportKind === 'ws') {
    client
      .waitReady()
      .then(ready => {
        Reflect.set(shim, 'ready', ready)
        Reflect.set(shim, '__backendReady', true)
      })
      .catch(error => {
        console.warn('[vite] bridge WebSocket not ready', error)
      })
  }
}
