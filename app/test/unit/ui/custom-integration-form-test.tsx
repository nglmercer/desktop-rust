import * as React from 'react'
import { act } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '../../helpers/ui/vitest-render'

const mocks = vi.hoisted(() => ({
  parse: vi.fn(),
  validatePath: vi.fn(),
}))

vi.mock('../../../src/lib/custom-integration', () => ({
  checkTargetPathArgument: (args: ReadonlyArray<string>) => args.some(arg => arg.includes('%TARGET_PATH%')),
  parseCustomIntegrationArgumentsForInput: mocks.parse,
  validateCustomIntegrationPath: mocks.validatePath,
  TargetPathArgument: '%TARGET_PATH%',
  WindowsExecutableExtensions: ['exe', 'com'],
}))
vi.mock('../../../src/ui/main-process-proxy', () => ({ showOpenDialog: vi.fn() }))

import { CustomIntegrationForm } from '../../../src/ui/preferences/custom-integration-form'

function deferred<T>() {
  let resolve: (value: T) => void = () => {}
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}

describe('custom integration form', () => {
  beforeEach(() => {
    mocks.parse.mockReset()
    mocks.validatePath.mockReset()
  })

  it('updates valid arguments synchronously and rejects missing placeholders', () => {
    mocks.parse.mockReturnValueOnce(['older']).mockReturnValueOnce(['new', '%TARGET_PATH%'])
    const onArgumentsChanged = vi.fn()
    render(<CustomIntegrationForm id="editor" path="" arguments="" onPathChanged={vi.fn()} onArgumentsChanged={onArgumentsChanged} />)
    const input = screen.getByLabelText('Arguments')
    fireEvent.change(input, { target: { value: 'older' } })
    fireEvent.change(input, { target: { value: 'new %TARGET_PATH%' } })
    expect(onArgumentsChanged).toHaveBeenCalledTimes(2)
    expect(onArgumentsChanged).toHaveBeenNthCalledWith(1, 'older')
    expect(onArgumentsChanged).toHaveBeenLastCalledWith('new %TARGET_PATH%')
    expect(input).toHaveProperty('value', 'new %TARGET_PATH%')
  })

  it('ignores path validation after a newer path or unmount', async () => {
    const first = deferred<{ isValid: boolean }>()
    const second = deferred<{ isValid: boolean }>()
    mocks.validatePath.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const onPathChanged = vi.fn()
    const view = render(<CustomIntegrationForm id="editor" path="" arguments="" onPathChanged={onPathChanged} onArgumentsChanged={vi.fn()} />)
    const input = screen.getByLabelText('Path')
    fireEvent.change(input, { target: { value: '/old' } })
    fireEvent.change(input, { target: { value: '/new' } })
    await act(async () => { second.resolve({ isValid: true }); await second.promise })
    view.unmount()
    await act(async () => { first.resolve({ isValid: true }); await first.promise })
    expect(onPathChanged).toHaveBeenCalledTimes(3)
    expect(onPathChanged).toHaveBeenNthCalledWith(1, '/old')
    expect(onPathChanged).toHaveBeenNthCalledWith(2, '/new')
    expect(onPathChanged).toHaveBeenLastCalledWith('/new', undefined)
  })
})
