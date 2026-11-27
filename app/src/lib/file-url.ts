import { bundleUrl } from '../web/bundle'

/**
 * Resolve a renderer path as a URL. Bundled resources remain page-relative;
 * real filesystem paths use the file: scheme.
 */
export function pathToFileURL(path: string): URL {
  const bundled = bundleUrl(path)
  if (bundled !== null) {
    return new URL(bundled)
  }

  const normalized = path.replace(/\\/g, '/')
  const absolute = normalized.startsWith('/') ? normalized : `/${normalized}`
  return new URL(
    `file://${absolute.split('/').map(encodeURIComponent).join('/')}`
  )
}
