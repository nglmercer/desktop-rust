// Where the renderer believes its bundled resources live: the value of
// `__dirname` in the web build (see vite.config.ts). Nothing exists at this
// path on disk; the Rust shell serves the same files from the page's origin.
export const BundleRoot = '/__desktop_app__'

/** The page-relative URL of a bundled resource, or null for a real path. */
export function bundleUrl(path: string): string | null {
  const normalized = path.replace(/\\/g, '/')
  if (normalized !== BundleRoot && !normalized.startsWith(`${BundleRoot}/`)) {
    return null
  }
  const relative = normalized.slice(BundleRoot.length + 1)
  return new URL(
    relative.split('/').map(encodeURIComponent).join('/'),
    document.baseURI
  ).toString()
}
