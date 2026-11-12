import { vi } from 'vitest'
import { JSDOM } from 'jsdom'
import packageInfo from '../package.json'

vi.mock('electron', () => ({
  shell: { openPath: vi.fn(async () => '') },
  ipcRenderer: { invoke: vi.fn(async () => {}), on: vi.fn() },
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
