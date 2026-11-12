import type { IMenuItem } from '../../lib/menu-item'

let lastPointer: { x: number; y: number } | undefined
let closeActive: (() => void) | undefined

if (typeof document !== 'undefined') {
  for (const eventName of ['pointerdown', 'contextmenu'] as const) {
    document.addEventListener(
      eventName,
      event => {
        if (
          event instanceof MouseEvent &&
          (event.clientX !== 0 || event.clientY !== 0)
        ) {
          lastPointer = { x: event.clientX, y: event.clientY }
        }
      },
      true
    )
  }
  document.addEventListener(
    'keydown',
    event => {
      if (
        event.key === 'ContextMenu' ||
        (event.shiftKey && event.key === 'F10')
      ) {
        lastPointer = undefined
      }
    },
    true
  )
}

/** Render a contextual menu in Rust-hosted webviews, returning the selected item path. */
export function showBrowserContextMenu(
  items: ReadonlyArray<IMenuItem>
): Promise<ReadonlyArray<number> | null> {
  closeActive?.()
  const previousFocus =
    document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null
  const bounds = previousFocus?.getBoundingClientRect()
  const anchor = lastPointer ?? { x: bounds?.left ?? 0, y: bounds?.bottom ?? 0 }
  lastPointer = undefined

  return new Promise(resolve => {
    const root = document.createElement('div')
    root.className = 'browser-context-menu-root'
    const menus: HTMLElement[] = []
    let finished = false

    const close = (
      selection: ReadonlyArray<number> | null = null,
      command?: string
    ) => {
      if (finished) return
      finished = true
      root.remove()
      document.removeEventListener('pointerdown', onOutside, true)
      document.removeEventListener('keydown', onKey, true)
      window.removeEventListener('blur', onBlur)
      if (closeActive === cancel) closeActive = undefined
      previousFocus?.focus({ preventScroll: true })
      try {
        if (command) document.execCommand(command)
      } finally {
        resolve(selection)
      }
    }
    const cancel = () => close()
    const onOutside = (event: Event) => {
      if (!root.contains(event.target as Node)) close()
    }
    const onBlur = () => close()
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        if (menus.length > 1) {
          menus.pop()?.remove()
          menus
            .at(-1)
            ?.querySelector<HTMLElement>('[aria-expanded="true"]')
            ?.focus()
          menus
            .at(-1)
            ?.querySelector<HTMLElement>('[aria-expanded="true"]')
            ?.setAttribute('aria-expanded', 'false')
        } else close()
      } else if (event.key === 'Tab') {
        close()
      } else if (
        event.key === 'ArrowDown' ||
        event.key === 'ArrowUp' ||
        event.key === 'Home' ||
        event.key === 'End'
      ) {
        const menu = menus.at(-1)
        const buttons = Array.from(
          menu?.querySelectorAll<HTMLButtonElement>(
            ':scope > button:not(:disabled)'
          ) ?? []
        )
        if (!buttons.length) return
        event.preventDefault()
        event.stopPropagation()
        const current = buttons.indexOf(
          document.activeElement as HTMLButtonElement
        )
        const next =
          event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? buttons.length - 1
              : (current +
                  (event.key === 'ArrowDown' ? 1 : -1) +
                  buttons.length) %
                buttons.length
        buttons[next].focus()
      } else if (event.key === 'ArrowLeft' && menus.length > 1) {
        event.preventDefault()
        event.stopPropagation()
        menus.pop()?.remove()
        const parent = menus
          .at(-1)
          ?.querySelector<HTMLElement>('[aria-expanded="true"]')
        parent?.setAttribute('aria-expanded', 'false')
        parent?.focus()
      } else if (
        event.key === 'ArrowRight' &&
        document.activeElement instanceof HTMLButtonElement &&
        document.activeElement.getAttribute('aria-haspopup') === 'menu'
      ) {
        event.preventDefault()
        event.stopPropagation()
        document.activeElement.click()
      }
    }

    const show = (
      entries: ReadonlyArray<IMenuItem>,
      path: number[],
      x: number,
      y: number
    ) => {
      const menu = document.createElement('div')
      menu.className = 'browser-context-menu'
      menu.setAttribute('role', 'menu')
      menu.style.left = `${x}px`
      menu.style.top = `${y}px`
      entries.forEach((entry, index) => {
        if (entry.type === 'separator') {
          const separator = document.createElement('div')
          separator.className = 'browser-context-menu-separator'
          separator.setAttribute('role', 'separator')
          menu.append(separator)
          return
        }
        if (entry.role === 'editMenu') {
          for (const [label, command] of [
            ['Undo', 'undo'],
            ['Redo', 'redo'],
            ['Cut', 'cut'],
            ['Copy', 'copy'],
            ['Paste', 'paste'],
            ['Select all', 'selectAll'],
          ]) {
            const button = document.createElement('button')
            button.type = 'button'
            button.setAttribute('role', 'menuitem')
            button.textContent = label
            button.addEventListener('click', () => close(null, command))
            menu.append(button)
          }
          return
        }
        const button = document.createElement('button')
        button.type = 'button'
        button.setAttribute(
          'role',
          entry.type === 'checkbox' ? 'menuitemcheckbox' : 'menuitem'
        )
        if (entry.type === 'checkbox')
          button.setAttribute('aria-checked', String(entry.checked === true))
        button.disabled = entry.enabled === false || entry.role !== undefined
        button.textContent = `${entry.type === 'checkbox' ? (entry.checked ? '✓ ' : '  ') : ''}${entry.label ?? ''}${entry.submenu ? ' ›' : ''}`
        if (entry.submenu) {
          button.setAttribute('aria-haspopup', 'menu')
          button.setAttribute('aria-expanded', 'false')
        }
        button.addEventListener('click', () => {
          if (entry.submenu) {
            while (menus.at(-1) !== menu) menus.pop()?.remove()
            menu
              .querySelector('[aria-expanded="true"]')
              ?.setAttribute('aria-expanded', 'false')
            button.setAttribute('aria-expanded', 'true')
            const rect = button.getBoundingClientRect()
            show(entry.submenu, [...path, index], rect.right, rect.top)
          } else close([...path, index])
        })
        menu.append(button)
      })
      root.append(menu)
      menus.push(menu)
      const rect = menu.getBoundingClientRect()
      menu.style.left = `${Math.max(0, Math.min(x, window.innerWidth - rect.width))}px`
      menu.style.top = `${Math.max(0, Math.min(y, window.innerHeight - rect.height))}px`
      menu.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus()
    }
    document.body.append(root)
    closeActive = cancel
    document.addEventListener('pointerdown', onOutside, true)
    document.addEventListener('keydown', onKey, true)
    window.addEventListener('blur', onBlur)
    show(items, [], anchor.x, anchor.y)
  })
}
