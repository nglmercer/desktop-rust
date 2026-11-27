/** The name for errors thrown because of an invalid URL. */
export const InvalidURLErrorName = 'invalid-url'

/** The name for errors thrown because of an invalid protocol. */
export const InvalidProtocolErrorName = 'invalid-protocol'

/**
 * Validate the URL for a GitHub Enterprise instance.
 *
 * Returns the validated URL, or throws if the URL cannot be validated.
 */
export function validateURL(address: string): string {
  // ensure user has specified text and not just whitespace
  // we will interact with this server so we can be fairly
  // relaxed here about what we accept for the server name
  const trimmed = address.trim()
  if (trimmed.length === 0) {
    const error = new Error('Unknown address')
    error.name = InvalidURLErrorName
    throw error
  }

  // E.g., if the user entered 'ghe.io', assume they're using https.
  address = /^[a-z][a-z0-9+.-]*:/i.test(trimmed)
    ? trimmed
    : `https://${trimmed}`

  let url: URL
  try {
    url = new URL(address)
  } catch {
    const error = new Error('Invalid URL')
    error.name = InvalidURLErrorName
    throw error
  }

  if (!url.host) {
    const error = new Error('Invalid URL')
    error.name = InvalidURLErrorName
    throw error
  }

  if (url.protocol !== 'https:') {
    const error = new Error('Invalid protocol')
    error.name = InvalidProtocolErrorName
    throw error
  }

  return address
}
