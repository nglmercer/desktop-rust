// `crypto` for the webview build: the browser implementation of Node's crypto
// module, plus `randomUUID`. That implementation has no `randomUUID`, and the
// Copilot SDK imports it (`import { randomUUID } from "node:crypto"`).

import { randomUUID } from './random-uuid'

export { randomUUID }

// The implementation comes from the `crypto-browserify` package (vite.config.ts).
// @ts-expect-error -- aliased by desktop-crypto-impl in vite.config.ts
export * from 'desktop-crypto-impl'

// The webview's global `crypto` may lack randomUUID (WebKit gives custom schemes
// no secure context). The app calls crypto.randomUUID() directly, so supply it
// once, as early as this module loads: the UI entry imports it first.
if (
  typeof globalThis.crypto !== 'undefined' &&
  typeof globalThis.crypto.randomUUID !== 'function'
) {
  Object.defineProperty(globalThis.crypto, 'randomUUID', {
    value: randomUUID,
    configurable: true,
  })
}
