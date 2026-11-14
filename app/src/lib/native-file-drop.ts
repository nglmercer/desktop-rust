const maximumPaths = 256
const maximumCharacters = 64 * 1024

/** Validate native webview drop events before using their paths. */
export function nativeFileDropPaths(
  event: Event
): ReadonlyArray<string> | null {
  if (!(event instanceof CustomEvent) || !Array.isArray(event.detail)) {
    return null
  }

  const paths: unknown[] = event.detail
  if (paths.length === 0 || paths.length > maximumPaths) {
    return null
  }

  let size = 0
  for (const path of paths) {
    if (typeof path !== 'string' || path.length === 0 || path.includes('\0')) {
      return null
    }
    size += path.length
    if (size > maximumCharacters) {
      return null
    }
  }

  return paths as string[]
}
