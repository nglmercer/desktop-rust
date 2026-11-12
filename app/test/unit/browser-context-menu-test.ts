import { afterEach, describe, expect, it, vi } from 'vitest'
import { showBrowserContextMenu } from '../../src/ui/lib/browser-context-menu'

afterEach(() => {
  document.querySelector('.browser-context-menu-root')?.remove()
  document.body.innerHTML = ''
  vi.restoreAllMocks()
})

describe('Rust webview context menu', () => {
  it('returns nested item paths and restores focus', async () => {
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()
    const result = showBrowserContextMenu([
      { label: 'Nested', submenu: [{ label: 'Action' }] },
    ])
    const parent = document.querySelector<HTMLButtonElement>(
      '[aria-haspopup="menu"]'
    )
    if (!parent) throw new Error('Missing submenu button')
    parent.click()
    const child = Array.from(
      document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')
    ).find(item => item.textContent === 'Action')
    if (!child) throw new Error('Missing nested action')
    child.click()
    await expect(result).resolves.toEqual([0, 0])
    expect(document.activeElement).toBe(input)
    expect(document.querySelector('[role="menu"]')).toBeNull()
  })

  it('supports keyboard navigation, disabled items, and dismissal', async () => {
    const result = showBrowserContextMenu([
      { label: 'Disabled', enabled: false },
      { label: 'One' },
      { type: 'checkbox', label: 'Two', checked: true },
    ])
    const items = document.querySelectorAll<HTMLButtonElement>('button')
    expect(items[0].disabled).toBe(true)
    expect(document.activeElement).toBe(items[1])
    document.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })
    )
    expect(document.activeElement).toBe(items[2])
    expect(items[2].getAttribute('aria-checked')).toBe('true')
    document.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })
    )
    await expect(result).resolves.toBeNull()
  })

  it('closes the previous menu and invokes edit roles after restoring focus', async () => {
    const input = document.createElement('textarea')
    document.body.append(input)
    input.focus()
    const first = showBrowserContextMenu([{ label: 'First' }])
    const second = showBrowserContextMenu([{ role: 'editMenu' }])
    await expect(first).resolves.toBeNull()
    const exec = vi.fn(() => true)
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: exec,
    })
    const selectAll = Array.from(
      document.querySelectorAll<HTMLButtonElement>('button')
    ).find(item => item.textContent === 'Select all')
    if (!selectAll) throw new Error('Missing edit action')
    selectAll.click()
    await expect(second).resolves.toBeNull()
    expect(document.activeElement).toBe(input)
    expect(exec).toHaveBeenCalledWith('selectAll')
    Reflect.deleteProperty(document, 'execCommand')
  })
})
