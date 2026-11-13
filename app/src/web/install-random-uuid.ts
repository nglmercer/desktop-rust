import { randomUUID } from './random-uuid'

// Some WebKit custom schemes are not secure contexts, so they expose
// getRandomValues but not randomUUID. Preserve native implementations.
if (
  typeof globalThis.crypto !== 'undefined' &&
  typeof globalThis.crypto.randomUUID !== 'function'
) {
  Object.defineProperty(globalThis.crypto, 'randomUUID', {
    value: randomUUID,
    configurable: true,
  })
}
