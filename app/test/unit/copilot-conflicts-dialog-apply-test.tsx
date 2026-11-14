import * as React from 'react'
import * as ReactDOM from 'react-dom'
import { act } from 'react-dom/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { MultiCommitOperationKind } from '../../src/models/multi-commit-operation'
import { Repository } from '../../src/models/repository'
import { WorkingDirectoryStatus } from '../../src/models/status'
import { Dispatcher } from '../../src/ui/dispatcher'
import { MultiCommitOperationConflictState } from '../../src/lib/app-state'

vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../src/lib/stores/copilot-store', () => ({
  formatReasoningEffort: () => '',
}))
vi.mock('../../src/ui/dialog', () => ({
  Dialog: (props: React.PropsWithChildren<{ onSubmit: () => void }>) => (
    <form
      onSubmit={event => {
        event.preventDefault()
        props.onSubmit()
      }}
    >
      {props.children}
    </form>
  ),
  DialogContent: (props: React.PropsWithChildren<{}>) => (
    <div>{props.children}</div>
  ),
  DialogFooter: (props: React.PropsWithChildren<{}>) => (
    <div>{props.children}</div>
  ),
  DialogError: (props: React.PropsWithChildren<{}>) => (
    <div role="alert">{props.children}</div>
  ),
}))
vi.mock('../../src/ui/dialog/header', () => ({
  DialogHeader: (props: React.PropsWithChildren<{}>) => (
    <div>{props.children}</div>
  ),
}))
vi.mock('../../src/ui/dialog/ok-cancel-button-group', () => ({
  OkCancelButtonGroup: (props: { okButtonDisabled: boolean }) => (
    <button type="submit" disabled={props.okButtonDisabled}>
      Continue
    </button>
  ),
}))
vi.mock('../../src/ui/lib/button', () => ({
  Button: (
    props: React.PropsWithChildren<{ disabled?: boolean; onClick?: () => void }>
  ) => (
    <button type="button" disabled={props.disabled} onClick={props.onClick}>
      {props.children}
    </button>
  ),
}))
vi.mock('../../src/ui/tab-bar', () => ({
  TabBar: (props: React.PropsWithChildren<{}>) => <div>{props.children}</div>,
  TabBarType: { Tabs: 'tabs' },
}))
vi.mock(
  '../../src/ui/multi-commit-operation/dialog/copilot-conflicts-resolution-summary',
  () => ({ CopilotConflictsResolutionSummary: () => null })
)
vi.mock(
  '../../src/ui/multi-commit-operation/dialog/copilot-conflicts-changes',
  () => ({ CopilotConflictsChanges: () => null })
)

import { CopilotConflictsDialog } from '../../src/ui/multi-commit-operation/dialog/copilot-conflicts-dialog'

describe('Copilot conflict application', () => {
  let container: HTMLDivElement | undefined
  afterEach(() => {
    if (container) {
      ReactDOM.unmountComponentAtNode(container)
      container.remove()
      container = undefined
    }
  })

  const setup = async (
    resolutions: React.ComponentProps<
      typeof CopilotConflictsDialog
    >['copilotResolutions']
  ) => {
    const apply = vi.fn()
    const manual = vi.fn()
    const proceed = vi.fn()
    const dispatcher = {
      applyCopilotConflictResolutions: apply,
      setMultiCommitOperationStepWithCopilotResolution: manual,
    } as unknown as Dispatcher
    const props: React.ComponentProps<typeof CopilotConflictsDialog> = {
      repository: { path: '/repository' } as Repository,
      dispatcher,
      conflictState: {
        manualResolutions: new Map(),
      } as MultiCommitOperationConflictState,
      workingDirectory: { files: [] } as unknown as WorkingDirectoryStatus,
      operationKind: MultiCommitOperationKind.Merge,
      copilotResolutions: resolutions,
      copilotResolutionSummary: null,
      copilotSkippedFiles: null,
      model: { modelName: 'Test model', reasoningEffort: undefined },
      resolvedExternalEditor: null,
      openFileInExternalEditor: vi.fn(),
      onContinueAfterConflicts: proceed,
      onAbort: vi.fn(),
      onDismissed: vi.fn(),
      emoji: new Map(),
    }
    const root = document.createElement('div')
    container = root
    document.body.appendChild(root)
    await act(async () => {
      ReactDOM.render(<CopilotConflictsDialog {...props} />, root)
    })
    return { root, apply, manual, proceed }
  }

  const submit = async (root: HTMLElement) => {
    await act(async () => {
      root
        .querySelector('form')
        ?.dispatchEvent(
          new Event('submit', { bubbles: true, cancelable: true })
        )
      await Promise.resolve()
    })
  }

  it('shows a stale-review error and blocks retries while retaining manual resolution', async () => {
    const view = await setup([
      { path: 'file', resolvedContent: 'result', reasoning: '' },
    ])
    view.apply.mockRejectedValueOnce(
      new Error('Conflict changed after suggestions were generated')
    )
    await submit(view.root)
    expect(view.root.querySelector('[role="alert"]')?.textContent).toContain(
      'Conflict changed'
    )
    expect(
      (view.root.querySelector('button[type="submit"]') as HTMLButtonElement)
        .disabled
    ).toBe(true)
    await submit(view.root)
    expect(view.apply).toHaveBeenCalledOnce()
    expect(view.proceed).not.toHaveBeenCalled()
    const manual = Array.from(view.root.querySelectorAll('button')).find(
      button => button.textContent?.includes('Switch to manual')
    )
    await act(async () => {
      manual?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    expect(view.manual).toHaveBeenCalledOnce()
  })

  it('refuses consumed suggestions even if a submit event is sent', async () => {
    const view = await setup(null)
    expect(view.root.querySelector('[role="alert"]')?.textContent).toContain(
      'no longer available'
    )
    await submit(view.root)
    expect(view.apply).not.toHaveBeenCalled()
    expect(view.proceed).not.toHaveBeenCalled()
  })

  it('continues once after successful application', async () => {
    const view = await setup([
      { path: 'file', resolvedContent: 'result', reasoning: '' },
    ])
    view.apply.mockResolvedValueOnce(undefined)
    view.proceed.mockResolvedValueOnce(undefined)
    await submit(view.root)
    expect(view.apply).toHaveBeenCalledOnce()
    expect(view.proceed).toHaveBeenCalledOnce()
  })
})
