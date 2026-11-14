import DOMPurify from 'dompurify'
import { isGitHubAssetVideo } from '../../lib/markdown-filters/video-url'

/** Sanitize markup and remove untrusted video sources before loading the frame. */
export function sanitizeMarkdownBody(
  markup: string,
  baseHref?: string
): string {
  // DOMPurify parses into an inert document. Keep its fragment in that same
  // document until video sources are checked: removing a live video later can
  // be too late to prevent its metadata request.
  const fragment = DOMPurify.sanitize(markup, { RETURN_DOM_FRAGMENT: true })
  const container = fragment.ownerDocument.createElement('div')
  container.appendChild(fragment)

  let base = document.baseURI
  if (baseHref !== undefined) {
    try {
      base = new URL(baseHref, base).href
    } catch {
      // An invalid base does not prevent absolute GitHub asset URLs.
    }
  }

  for (const video of container.querySelectorAll('video')) {
    const source = video.getAttribute('src')
    if (source === null || !isGitHubAssetVideo(source, base)) {
      video.remove()
      continue
    }

    for (const child of video.querySelectorAll('source, track')) {
      const childSource = child.getAttribute('src')
      if (childSource === null || !isGitHubAssetVideo(childSource, base)) {
        child.remove()
      }
    }
  }
  return container.innerHTML
}
