import { BundleRoot, bundleUrl } from '../../web/bundle'

/** Read a resource served by the Rust host from the renderer bundle. */
export async function readBundledTemplate(path: string): Promise<string> {
  const url = bundleUrl(`${BundleRoot}/${path}`)
  if (url === null) {
    throw new Error('Invalid bundled template path')
  }
  const response = await fetch(url)
  if (!response.ok) {
    throw new Error(`Unable to load bundled template (${response.status})`)
  }
  return response.text()
}

/** List the gitignore templates included in the renderer bundle. */
export async function listBundledGitIgnores(): Promise<ReadonlyArray<string>> {
  const url = new URL('__bundle', document.baseURI)
  url.searchParams.set('path', 'static/gitignore')
  const response = await fetch(url)
  if (!response.ok) {
    throw new Error(
      `Unable to list bundled gitignore templates (${response.status})`
    )
  }
  const entry: unknown = await response.json()
  if (
    entry === null ||
    typeof entry !== 'object' ||
    !('kind' in entry) ||
    entry.kind !== 'directory' ||
    !('entries' in entry) ||
    !Array.isArray(entry.entries)
  ) {
    throw new Error('Invalid bundled gitignore template list')
  }
  return entry.entries.map((item: unknown) => {
    if (
      item === null ||
      typeof item !== 'object' ||
      !('name' in item) ||
      typeof item.name !== 'string' ||
      !item.name ||
      item.name === '.' ||
      item.name === '..' ||
      /[\\/\0]/.test(item.name)
    ) {
      throw new Error('Invalid bundled gitignore template name')
    }
    return item.name
  })
}
