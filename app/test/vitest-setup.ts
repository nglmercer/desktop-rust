import 'fake-indexeddb/auto'
import { expect, vi } from 'vitest'
import { JSDOM } from 'jsdom'
import packageInfo from '../package.json'

vi.mock('../src/web/host-bridge', () => ({
  hostShell: { readShortcutLink: vi.fn(() => ({})) },
  hostIpc: { invoke: vi.fn(async () => {}), on: vi.fn() },
}))

Object.assign(globalThis, {
  __DEV__: false,
  __TEST__: true,
  __DEV_SECRETS__: false,
  __APP_NAME__: packageInfo.productName,
  __APP_VERSION__: packageInfo.version,
  __RELEASE_CHANNEL__: 'development',
  __UPDATES_URL__: '',
  __SHA__: 'test',
  __DARWIN__: process.platform === 'darwin',
  __WIN32__: process.platform === 'win32',
  __LINUX__: process.platform === 'linux',
  log: { error() {}, warn() {}, info() {}, debug() {} },
})

// Recent Node versions expose their own optional localStorage global. Use the
// isolated JSDOM storage consistently, including when Node's is unavailable.
vi.stubGlobal(
  'localStorage',
  new JSDOM('', { url: 'http://localhost/' }).window.localStorage
)

// Opt-in: run every test file against a real Rust server, so the renderer takes
// its Rust code paths instead of the Node fallbacks (see rust/ROADMAP.md, R1).
if (process.env.DESKTOP_TEST_RUST_BACKEND === '1') {
  const harness = await import('./helpers/rust-backend')
  // These tests install their own `window`, bridge or transport, which is what
  // the harness does too, so they cannot share it.
  const ownsItsBackend =
    /(?:^|\/)(?:script\/|web-open-path-test|web-host-bridge-loader-test|web-fs-|backend-|pull-hook-callbacks-test)/.test(
      expect.getState().testPath ?? ''
    )
  if (harness.rustBackendAvailable && !ownsItsBackend) {
    harness.useRustBackend()
  } else {
    // Processes this test spawns must not start a backend either.
    delete process.env.DESKTOP_TEST_RUST_BACKEND
  }
}
