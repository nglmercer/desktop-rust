/**
 * Injected before UI entry to expose window.__desktop bridge to electron shim.
 * This mirrors rust/crates/desktop-app/web/ipc-shim.js but for Vite dev.
 * In production tao/wry host, ipc-shim.js is already injected via wry initialization script.
 * This file ensures Vite dev server also has bridge via loopback WebSocket if available.
 */
if (typeof window !== 'undefined' && !(window as any).__desktop) {
  // Minimal ready marker; real transport attaches here when served by desktop-bridge or tao host.
  (window as any).__desktop = {
    invoke: async (channel: string) => { throw new Error(`[vite] bridge not attached — run via desktop-rs/treat`) },
    send: () => {},
    on: () => {},
    emit: () => {},
    resolve: () => {},
  }
}
