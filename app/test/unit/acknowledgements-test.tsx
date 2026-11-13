import * as React from 'react'
import * as ReactDOM from 'react-dom'
import { act } from 'react-dom/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'

const readNotices = vi.hoisted(() => vi.fn())
const getAppPath = vi.hoisted(() => vi.fn(async () => '/app'))
vi.mock('fs', async importOriginal => ({
  ...(await importOriginal<typeof import('fs')>()),
  readFile: readNotices,
}))
vi.mock('../../src/ui/lib/app-proxy', () => ({
  getAppPath,
}))
vi.mock('../../src/ui/lib/link-button', () => ({
  LinkButton: (props: React.PropsWithChildren<{ uri?: string }>) => (
    <a href={props.uri}>{props.children}</a>
  ),
}))
vi.mock('../../src/ui/dialog', () => ({
  Dialog: (props: React.PropsWithChildren<{}>) => <div>{props.children}</div>,
  DialogContent: (props: React.PropsWithChildren<{}>) => (
    <div>{props.children}</div>
  ),
  DefaultDialogFooter: () => null,
}))

import { Acknowledgements } from '../../src/ui/acknowledgements/acknowledgements'

describe('acknowledgements', () => {
  let root: HTMLDivElement | undefined
  afterEach(() => {
    if (root) {
      ReactDOM.unmountComponentAtNode(root)
      root.remove()
      root = undefined
    }
    readNotices.mockReset()
    getAppPath.mockReset()
    getAppPath.mockResolvedValue('/app')
  })

  const render = async () => {
    const container = document.createElement('div')
    root = container
    document.body.appendChild(container)
    await act(async () => {
      ReactDOM.render(
        <Acknowledgements applicationVersion="1" onDismissed={() => {}} />,
        container
      )
    })
  }

  it('shows Desktop separately and retains the first alphabetic dependency', async () => {
    readNotices.mockImplementation((_path, _encoding, callback) =>
      callback(
        null,
        JSON.stringify({
          '@example/first@1': { license: 'MIT' },
          'desktop@1': { sourceText: 'Desktop license' },
          'z-last@1': { license: 'ISC' },
        })
      )
    )
    await render()
    expect(root?.textContent).toContain('Desktop license')
    expect(root?.textContent).toContain('@example/first@1')
    expect(root?.textContent).toContain('z-last@1')
    expect(root?.textContent).not.toContain('desktop@1')
  })

  it('shows an error instead of waiting forever when notices cannot be read', async () => {
    readNotices.mockImplementation((_path, _encoding, callback) =>
      callback(new Error('unavailable'))
    )
    await render()
    expect(root?.textContent).toContain('License notices could not be loaded.')
  })

  it('shows an error if the application path cannot be obtained', async () => {
    getAppPath.mockRejectedValueOnce(new Error('unavailable'))
    await render()
    expect(readNotices).not.toHaveBeenCalled()
    expect(root?.textContent).toContain('License notices could not be loaded.')
  })

  it('shows an error for an invalid notice file', async () => {
    readNotices.mockImplementation((_path, _encoding, callback) =>
      callback(null, 'not json')
    )
    await render()
    expect(root?.textContent).toContain('License notices could not be loaded.')
  })

  it('rejects malformed notice entries', async () => {
    readNotices.mockImplementation((_path, _encoding, callback) =>
      callback(null, '{"package@1":null}')
    )
    await render()
    expect(root?.textContent).toContain('License notices could not be loaded.')
  })

  it('handles a notice file without the Desktop entry', async () => {
    readNotices.mockImplementation((_path, _encoding, callback) =>
      callback(null, '{"example@1":{"license":"MIT"}}')
    )
    await render()
    expect(root?.textContent).toContain('example@1')
  })
})
