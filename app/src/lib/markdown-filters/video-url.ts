/** Return whether a video URL points to GitHub's user-asset origin. */
export function isGitHubAssetVideo(value: string, base?: string): boolean {
  try {
    const url = base === undefined ? new URL(value) : new URL(value, base)
    return (
      url.protocol === 'https:' &&
      url.hostname === 'user-images.githubusercontent.com' &&
      url.port === '' &&
      url.username === '' &&
      url.password === ''
    )
  } catch {
    return false
  }
}
