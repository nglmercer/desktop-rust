import * as React from 'react'
import * as ReactDOM from 'react-dom'
import { act } from 'react-dom/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'

const saveSetting = vi.hoisted(() => vi.fn())
vi.mock('../../src/lib/git/config', () => ({
  setGlobalConfigValue: saveSetting,
}))
vi.mock('../../src/lib/git', () => ({
  isConfigFileLockError: () => false,
  parseConfigLockFilePathFromError: () => null,
}))
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))
vi.mock('../../src/lib/email', () => ({ lookupPreferredEmail: () => '' }))
vi.mock('../../src/ui/lib/config-lock-file-exists', () => ({
  ConfigLockFileExists: () => null,
}))
vi.mock('../../src/ui/lib/git-email-not-found-warning', () => ({
  GitEmailNotFoundWarning: () => null,
}))
vi.mock('../../src/ui/history/commit-list-item', () => ({
  CommitListItem: () => null,
}))
import { ConfigureGitUser } from '../../src/ui/lib/configure-git-user'

describe('Git identity setup', () => {
  let root: HTMLDivElement | undefined
  afterEach(() => {
    if (root) {
      ReactDOM.unmountComponentAtNode(root)
      root.remove()
      root = undefined
    }
    saveSetting.mockReset()
  })
  it('does not advance after a failed or partially completed save and permits a retry', async () => {
    const container = document.createElement('div')
    root = container
    document.body.appendChild(container)
    const ref = React.createRef<ConfigureGitUser>()
    const onSave = vi.fn()
    await act(async () => {
      ReactDOM.render(
        <ConfigureGitUser
          ref={ref}
          accounts={[]}
          globalUserName={undefined}
          globalUserEmail={undefined}
          onSave={onSave}
        />,
        container
      )
    })
    await act(async () => {
      ref.current?.setState({
        manualName: 'Alice',
        manualEmail: 'alice@example.invalid',
      })
    })
    saveSetting
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('bridge disconnected'))
    const form = root.querySelector('form')
    expect(form).not.toBeNull()
    await act(async () => {
      form?.dispatchEvent(
        new Event('submit', { bubbles: true, cancelable: true })
      )
      await Promise.resolve()
    })
    expect(saveSetting).toHaveBeenCalledTimes(2)
    expect(onSave).not.toHaveBeenCalled()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain(
      'Some changes may have been saved'
    )
    saveSetting.mockResolvedValue(undefined)
    await act(async () => {
      form?.dispatchEvent(
        new Event('submit', { bubbles: true, cancelable: true })
      )
      await Promise.resolve()
    })
    expect(onSave).toHaveBeenCalledOnce()
    expect(root.querySelector('[role="alert"]')).toBeNull()
  })
})
