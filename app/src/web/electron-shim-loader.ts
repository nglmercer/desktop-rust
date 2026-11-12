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

if (typeof window !== 'undefined' && !(window as any).__desktop) {
  const client = backend()
  const subscriptions = new Map<
    string,
    Map<(...args: unknown[]) => void, () => void>
  >()

  ;(window as any).__desktop = {
    __transport: client.transportKind,
    invoke: async (channel: string, ...args: unknown[]) => {
      if (client.transportKind === 'none') {
        throw new Error(
          `[vite] bridge not attached — run via desktop-rs serve or the Tao/Wry shell`
        )
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
        // Simplex over WS: fire-and-forget snapshot/preferences pushes are
        // server-driven; client sends are RPCs, so a stray `send` is a no-op.
        return
      }
      ;(window as any).ipc?.postMessage?.(JSON.stringify({ channel, args }))
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
      return (window as any).__desktop
    },
    removeListener: (
      channel: string,
      listener: (...args: unknown[]) => void
    ) => {
      const listeners = subscriptions.get(channel)
      listeners?.get(listener)?.()
      listeners?.delete(listener)
      if (listeners?.size === 0) subscriptions.delete(channel)
      return (window as any).__desktop
    },
    removeAllListeners: (channel?: string) => {
      for (const [event, listeners] of subscriptions) {
        if (channel !== undefined && channel !== event) continue
        for (const unsubscribe of listeners.values()) unsubscribe()
        subscriptions.delete(event)
      }
      return (window as any).__desktop
    },
    emit: () => {},
    resolve: () => {},
    // Exposed for `pendingLimitFromHealth` in `app/src/backend/client.ts`.
    pendingPromises: client.capabilities().max_pending,
  }

  // Prefer the WebSocket bridge when we are not in the Wry shell.
  if (client.transportKind === 'ws') {
    client
      .waitReady()
      .then(ready => {
        ;(window as any).__desktop.ready = ready
        ;(window as any).__desktop.__backendReady = true
      })
      .catch(error => {
        console.warn('[vite] bridge WebSocket not ready', error)
      })
  }
}
