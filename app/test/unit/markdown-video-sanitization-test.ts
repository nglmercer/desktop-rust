import { describe, expect, it } from 'vitest'
import DOMPurify from 'dompurify'
import { isGitHubAssetVideo } from '../../src/lib/markdown-filters/video-url'
import { VideoLinkFilter } from '../../src/lib/markdown-filters/video-link-filter'
import { VideoTagFilter } from '../../src/lib/markdown-filters/video-tag-filter'
import { sanitizeMarkdownBody } from '../../src/ui/lib/sanitize-markdown-body'

describe('Markdown video sanitization', () => {
  it('preserves ordinary sanitized Markdown markup', () => {
    const markup =
      '<p>Hi <a href="../issue">there</a> &amp; <strong>more</strong></p><pre><code>const x = 1</code></pre><table><tr><td>cell</td></tr></table>'
    expect(sanitizeMarkdownBody(markup)).toBe(DOMPurify.sanitize(markup))
  })

  it('accepts only the exact HTTPS user asset host', () => {
    expect(
      isGitHubAssetVideo('https://user-images.githubusercontent.com/a.mp4')
    ).toBe(true)
    expect(
      isGitHubAssetVideo(
        '/a.mp4',
        'https://user-images.githubusercontent.com/repo'
      )
    ).toBe(true)
    for (const url of [
      'http://user-images.githubusercontent.com/a.mp4',
      'https://user-images.githubusercontent.com.evil.test/a.mp4',
      'https://user-images.githubusercontent.com@evil.test/a.mp4',
      'https://user-images.githubusercontent.com:444/a.mp4',
      'javascript:alert(1)',
      '/relative.mp4',
    ]) {
      expect(isGitHubAssetVideo(url), url).toBe(false)
    }
  })

  it('removes untrusted videos and nested sources before mounting', () => {
    const markup = `
      <p>Keep <a href="../issue">relative link</a></p>
      <video src="https://evil.test/a.mp4" preload="metadata"></video>
      <video src="https://user-images.githubusercontent.com.evil.test/a.mp4"></video>
      <video src="https://user-images.githubusercontent.com/a.mp4" controls>
        <source src="https://evil.test/b.mp4">
        <source src="https://user-images.githubusercontent.com/b.mp4">
        <track src="https://evil.test/a.vtt">
        <track src="https://user-images.githubusercontent.com/a.vtt">
      </video>`
    const container = document.createElement('div')
    container.innerHTML = sanitizeMarkdownBody(
      markup,
      'https://github.com/owner/repo'
    )
    expect(container.querySelector('a')?.getAttribute('href')).toBe('../issue')
    const videos = container.querySelectorAll('video')
    expect(videos).toHaveLength(1)
    expect(videos[0].getAttribute('src')).toBe(
      'https://user-images.githubusercontent.com/a.mp4'
    )
    expect(
      Array.from(videos[0].querySelectorAll('source, track'), node =>
        node.getAttribute('src')
      )
    ).toEqual([
      'https://user-images.githubusercontent.com/b.mp4',
      'https://user-images.githubusercontent.com/a.vtt',
    ])
  })

  it('resolves relative video and child sources against the supplied base', () => {
    const markup =
      '<video src="./a.mp4"><source src="../b.mp4"><track src="https://evil.test/x.vtt"></video>'
    const container = document.createElement('div')
    container.innerHTML = sanitizeMarkdownBody(
      markup,
      'https://user-images.githubusercontent.com/repo/path/'
    )
    expect(container.querySelector('video')?.getAttribute('src')).toBe(
      './a.mp4'
    )
    expect(container.querySelector('source')?.getAttribute('src')).toBe(
      '../b.mp4'
    )
    expect(container.querySelector('track')).toBeNull()
    expect(sanitizeMarkdownBody(markup, 'https://example.com/')).not.toContain(
      '<video'
    )
  })

  it('applies the same host restriction when creating video from a link', async () => {
    const filter = new VideoLinkFilter()
    const tagFilter = new VideoTagFilter()
    const paragraph = document.createElement('p')
    paragraph.innerHTML =
      '<a href="https://user-images.githubusercontent.com.evil.test/a.mp4">link</a>'
    expect(await filter.filter(paragraph)).toBeNull()
    paragraph.innerHTML =
      '<a href="https://user-images.githubusercontent.com/a.mp4">link</a>'
    const generated = await filter.filter(paragraph)
    expect(generated).toHaveLength(1)
    const video = generated?.[0]
    if (!(video instanceof HTMLVideoElement)) {
      throw new Error('Expected a video')
    }
    expect(video.src).toBe('https://user-images.githubusercontent.com/a.mp4')
    const malicious = document.createElement('video')
    malicious.src = 'https://user-images.githubusercontent.com.evil.test/a.mp4'
    expect(await tagFilter.filter(malicious)).toEqual([])
  })
})
