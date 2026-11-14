// Preloaded (`--require`) by the Node test runner before any test file.
'use strict'

require('fake-indexeddb/auto')
require('global-jsdom/register')
const Module = require('node:module')
const path = require('node:path')
const { mock } = require('node:test')

// Node may expose a localStorage global without a backing file. Always use
// this test process's JSDOM storage, as the Vitest setup does.
Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  writable: true,
  value: window.localStorage,
})

// These constants are defined by Vite at build time, but since tests aren't
// built with Vite we need to make sure these exist at runtime.
const packageInfo = require('../package.json')

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
  log: {
    error: () => {},
    warn: () => {},
    info: () => {},
    debug: () => {},
  },

  // The following types are part of the WebWorker support in Node.js and are a
  // common source of hangs in tests due to libraries creating them but not
  // properly cleaning them up. See for example
  // https://github.com/facebook/react/issues/20756, and
  // https://github.com/dexie/Dexie.js/pull/1577.
  //
  // We've upgraded Dexie already but react-dom is a bigger beast and we don't
  // need any of them to run our tests so we just delete them here. In fact,
  // this is exactly what the react-16-node-hanging-test-fix patch does, see
  // https://www.npmjs.com/package/react-16-node-hanging-test-fix?activeTab=code
  MessageChannel: undefined,
  MessagePort: undefined,
  BroadcastChannel: undefined,
})

// Electron and the native Node addons are no longer installed; the renderer's
// web build aliases them to shims, so Node tests resolve them the same way.
const shimmed = new Map(
  [
    ['host-bridge', '../src/web/host-bridge.ts'],
    ['registry-js', '../src/web/registry-js.ts'],
    ['keytar', '../src/web/keytar.ts'],
    // The vendored packages ship TypeScript sources; their `dist` is only
    // produced by native install scripts that are not run here.
    ['desktop-notifications', '../../vendor/desktop-notifications/lib/index.ts'],
    ['windows-argv-parser', '../../vendor/windows-argv-parser/index.ts'],
  ].map(([name, file]) => [name, path.resolve(__dirname, file)])
)
mock.module(shimmed.get('host-bridge'), {
  namedExports: {
    shell: {
      // Consumed through `app-shell`'s `openPath`, which binds this eagerly at
      // import time, so it has to exist here for tests to be able to mock it.
      openPath: async () => '',
    },
    ipcRenderer: {
      invoke: mock.fn(async () => {}),
      on: mock.fn(() => {}),
    },
    webUtils: { getPathForFile: () => '' },
  },
})

// Redirect the shimmed packages. This wraps `Module._load` after the mock
// above is installed so the mock still sees the shim's real path.
const load = Module._load
Module._load = function (request, ...rest) {
  return load.call(this, shimmed.get(request) ?? request, ...rest)
}

// `mock.module()` resolves its specifier with the ES module resolver, which
// needs file extensions. Probe TypeScript sources like the previous loader did.
const { pathToFileURL, fileURLToPath } = require('node:url')
const fs = require('node:fs')
Module.registerHooks({
  resolve(specifier, context, nextResolve) {
    const shim = shimmed.get(specifier)
    if (shim !== undefined) {
      return { url: pathToFileURL(shim).href, shortCircuit: true }
    }
    try {
      return nextResolve(specifier, context)
    } catch (error) {
      const relative = specifier.startsWith('.') || path.isAbsolute(specifier)
      if (!relative || context.parentURL === undefined) {
        throw error
      }
      const base = path.resolve(
        path.dirname(fileURLToPath(context.parentURL)),
        specifier
      )
      for (const candidate of [
        `${base}.ts`,
        `${base}.tsx`,
        path.join(base, 'index.ts'),
        path.join(base, 'index.tsx'),
      ]) {
        if (fs.existsSync(candidate)) {
          return { url: pathToFileURL(candidate).href, shortCircuit: true }
        }
      }
      throw error
    }
  },
})
