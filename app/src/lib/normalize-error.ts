/**
 * Normalize an unknown thrown value into the serializable error shape used by
 * logs and crash reports.
 *
 * The renderer is now built as hashed Vite chunks whose production source maps
 * are stored outside the application bundle. The legacy synchronous
 * source-map-support loader could never resolve those maps, so this helper
 * intentionally preserves the original stack rather than pretending to remap it.
 */
export function normalizeError(error: unknown): Error {
  // Guard against `throw "Foo"` which is totally a thing that can happen
  if (typeof error === 'string') {
    return {
      name: 'StringError',
      message: error,
    }
  }

  let message

  if (error && typeof error === 'object' && 'message' in error) {
    if (error.message === null) {
      message = 'null'
    } else if (error.message === undefined) {
      message = 'undefined'
    } else if (typeof error.message === 'string') {
      message = error.message
    } else if (typeof error.message === 'object') {
      message = JSON.stringify(
        error.message,
        Object.getOwnPropertyNames(error.message)
      )
    } else {
      message = `${error.message}`
    }
  } else {
    message = '[Unknown]'
  }

  return {
    name:
      error &&
      typeof error === 'object' &&
      'name' in error &&
      typeof error.name === 'string'
        ? error.name
        : '[Unknown]',
    message,
    stack: error instanceof Error ? error.stack : undefined,
  }
}
