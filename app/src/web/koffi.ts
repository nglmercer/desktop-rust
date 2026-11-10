// `koffi` (native FFI) for the webview build. The Copilot SDK only loads it
// for its in-process runtime host; the stdio connection the app uses never
// reaches it, so loading succeeds and calling anything reports why.
const unavailable = () => {
  throw new Error('Native FFI is not available in the webview')
}

export default new Proxy({}, { get: () => unavailable })
