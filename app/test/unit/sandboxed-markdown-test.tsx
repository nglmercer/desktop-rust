import * as React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render, waitFor } from '../helpers/ui/vitest-render'
import { SandboxedMarkdown } from '../../src/ui/lib/sandboxed-markdown'

const mocks = vi.hoisted(() => ({ fetch: vi.fn() }))

const emoji = new Map()
const originalLinux = __LINUX__
const props = {
  emoji,
  underlineLinks: false,
  title: 'Markdown',
}

beforeEach(() => {
  vi.stubGlobal('__LINUX__', true)
  Object.defineProperty(window, '__desktop', {
    configurable: true,
    value: { invoke: vi.fn(), on: vi.fn() },
  })
  vi.stubGlobal('fetch', mocks.fetch)
  mocks.fetch.mockImplementation(() => Promise.resolve(new Response('')))
})

afterEach(() => {
  Reflect.deleteProperty(window, '__desktop')
  mocks.fetch.mockReset()
  Object.assign(globalThis, { __LINUX__: originalLinux })
})

describe('sandboxed markdown in WebKitGTK', () => {
  it('retains the original sandbox outside the Linux native shell', () => {
    Reflect.deleteProperty(window, '__desktop')
    const view = render(<SandboxedMarkdown {...props} markdown="browser" />)
    expect(
      view.container.querySelector('iframe')?.getAttribute('sandbox')
    ).toBe('allow-same-origin')
    view.unmount()

    vi.stubGlobal('__LINUX__', false)
    Object.defineProperty(window, '__desktop', {
      configurable: true,
      value: { invoke: vi.fn(), on: vi.fn() },
    })
    const otherPlatform = render(
      <SandboxedMarkdown {...props} markdown="other host" />
    )
    expect(
      otherPlatform.container.querySelector('iframe')?.getAttribute('sandbox')
    ).toBe('allow-same-origin')
  })

  it('renders sanitized content, intercepts links and updates its base', async () => {
    const onLink = vi.fn()
    const view = render(
      <SandboxedMarkdown
        {...props}
        markdown={
          '# Hello 🐙\n\n[absolute](https://example.invalid/a)\n\n[relative](/b)\n\n<img src=x onerror="window.bad=true">'
        }
        onMarkdownLinkClicked={onLink}
      />
    )
    const frame = view.container.querySelector('iframe')!
    await waitFor(() =>
      expect(frame.contentDocument?.body.textContent).toContain('Hello 🐙')
    )
    expect(frame.getAttribute('sandbox')).toContain('allow-scripts')
    expect(frame.contentDocument?.querySelector('[onerror]')).toBeNull()
    const click = (label: string) => {
      const doc = frame.contentDocument!
      const anchor = [...doc.querySelectorAll('a')].find(
        a => a.textContent === label
      )!
      anchor.dispatchEvent(
        new MouseEvent('click', { bubbles: true, cancelable: true })
      )
    }
    click('absolute')
    click('relative')
    expect(onLink.mock.calls).toEqual([['https://example.invalid/a']])

    view.rerender(
      <SandboxedMarkdown
        {...props}
        markdown={'# Updated\n\n[relative](/b)'}
        baseHref="https://example.invalid/root/"
        onMarkdownLinkClicked={onLink}
      />
    )
    await waitFor(() =>
      expect(frame.contentDocument?.body.textContent).toContain('Updated')
    )
    click('relative')
    expect(onLink.mock.calls).toEqual([
      ['https://example.invalid/a'],
      ['https://example.invalid/b'],
    ])
  })

  it('does not replace a newer render when an older stylesheet read finishes late', async () => {
    let finishOld: () => void = () => {}
    mocks.fetch.mockImplementationOnce(
      () =>
        new Promise<Response>(resolve => {
          finishOld = () => resolve(new Response(''))
        })
    )
    const view = render(<SandboxedMarkdown {...props} markdown="old" />)
    const frame = view.container.querySelector('iframe')!
    view.rerender(<SandboxedMarkdown {...props} markdown="new" />)
    await waitFor(() =>
      expect(frame.contentDocument?.body.textContent).toContain('new')
    )
    finishOld()
    await Promise.resolve()
    expect(frame.contentDocument?.body.textContent).toContain('new')
    expect(frame.contentDocument?.body.textContent).not.toContain('old')
  })
})
