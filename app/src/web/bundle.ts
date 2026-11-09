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

export interface IBundleEntry {
  readonly kind: 'file' | 'directory'
  readonly size?: number
  readonly entries?: ReadonlyArray<{ name: string; isDirectory: boolean }>
}

const entryUrl = (path: string) => {
  const relative = path.replace(/\\/g, '/').slice(BundleRoot.length + 1)
  return new URL(
    `__bundle?path=${encodeURIComponent(relative)}`,
    document.baseURI
  ).toString()
}

/** Describe a bundled resource, or null when there is none at `path`. */
export async function bundleEntry(path: string): Promise<IBundleEntry | null> {
  return (await fetch(entryUrl(path))).json()
}

export function bundleEntrySync(path: string): IBundleEntry | null {
  const request = new XMLHttpRequest()
  request.open('GET', entryUrl(path), false)
  request.send()
  return JSON.parse(request.responseText)
}
