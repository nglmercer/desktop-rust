// `url` for the webview build: the legacy parse/format API from the `url`
// package, plus the file-URL helpers that package does not provide.
// @ts-ignore -- aliased to the `url` npm package in vite.config.ts
import legacy from 'desktop-url-impl'

/**
 * Where the renderer believes its bundled resources live (`__dirname` in the
 * web build). The Rust shell serves them from the page's own origin.
 */
const BundleRoot = '/__desktop_app__'

export const parse: (url: string, parseQuery?: boolean, slashes?: boolean) => any =
  legacy.parse
export const format: (url: any) => string = legacy.format
export const resolve: (from: string, to: string) => string = legacy.resolve
export const resolveObject = legacy.resolveObject
export const Url = legacy.Url
export const URL = globalThis.URL
export const URLSearchParams = globalThis.URLSearchParams

export function pathToFileURL(path: string): URL {
  const normalized = path.replace(/\\/g, '/')
  if (normalized === BundleRoot || normalized.startsWith(`${BundleRoot}/`)) {
    const relative = normalized.slice(BundleRoot.length + 1)
    return new globalThis.URL(
      relative.split('/').map(encodeURIComponent).join('/'),
      document.baseURI
    )
  }
  const absolute = normalized.startsWith('/') ? normalized : `/${normalized}`
  return new globalThis.URL(
    'file://' + absolute.split('/').map(encodeURIComponent).join('/')
  )
}

export function fileURLToPath(url: string | URL): string {
  const parsed = typeof url === 'string' ? new globalThis.URL(url) : url
  if (parsed.protocol !== 'file:') {
    throw new TypeError('The URL must be of scheme file')
  }
  const path = decodeURIComponent(parsed.pathname)
  // file:///C:/dir -> C:\dir
  return /^\/[a-zA-Z]:/.test(path) ? path.slice(1).replace(/\//g, '\\') : path
}

export default {
  parse, format, resolve, resolveObject, Url, URL, URLSearchParams,
  pathToFileURL, fileURLToPath,
}
