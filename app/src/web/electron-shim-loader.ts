/**
 * Injected before the UI entry. Mirrors `rust/crates/desktop-app/web/ipc-
 * shim.js` but for the Vite dev / loopback-bridge path. In the production
 * Tao/Wry host, `ipc-shim.js` is already injected via the wry
 * initialization script and `window.__desktop` exists — this loader is a
 * no-op there. Otherwise it installs a minimal `__desktop` marker and,
 * when the page was served by the Rust bridge (`desktop-rs serve`, Termux
 * or the loopback server), attaches the typed WebSocket transport from
 * `app/src/backend/client.ts` behind the same `invoke`/`send`/`on`
 * surface the shims (`electron.ts`, `child_process.ts`, `dugite.ts`,
 * `keytar.ts`, `fs.ts`) expect.
 *
 * The marker `invoke` throws `[vite] bridge not attached` until the WebSocket
 * transport signals `ready`; callers that probe `__desktop` can distinguish
 * "shell present but not ready" from "no shell".
 */
import { backend } from '../backend/client'

if (typeof window !== 'undefined' && !(window as any).__desktop) {
  const client = backend()

  ;(window as any).__desktop = {
    invoke: async (channel: string, ...args: unknown[]) => {
      if (client.transportKind === 'none') {
        throw new Error(`[vite] bridge not attached — run via desktop-rs serve or the Tao/Wry shell`)
      }
      // `desktop.backend.*` channels map to the typed client; legacy
      // `desktop.*` / `desktop.io.*` channels are forwarded through the
      // transport unchanged (the Rust shell/bridge owns them).
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
        // Map bridge events to ipc listeners.
        return client as unknown as { on?: unknown }
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
