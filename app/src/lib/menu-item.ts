import type { HostMenuItemOptions } from './host-types'
import { showBrowserContextMenu } from '../ui/lib/browser-context-menu'

export interface IMenuItem {
  /** The user-facing label. */
  readonly label?: string

  /** The action to invoke when the user selects the item. */
  readonly action?: () => void

  /** The type of item. */
  readonly type?: 'separator' | 'checkbox'

  /** Is the menu item checked? Only applies to checkbox type. */
  readonly checked?: boolean

  /** Is the menu item enabled? Defaults to true. */
  readonly enabled?: boolean

  /**
   * The predefined behavior of the menu item.
   *
   * When specified the click property will be ignored.
   */
  readonly role?: HostMenuItemOptions['role']

  /**
   * Submenu that will appear when hovering this menu item.
   */
  readonly submenu?: ReadonlyArray<this>
}

/**
 * Converts accelerator modifiers to their platform specific
 * name or symbol.
 *
 * Example: CommandOrControl becomes either '⌘' or 'Ctrl' depending on platform.
 *
 */
export function getPlatformSpecificNameOrSymbolForModifier(
  modifier: string
): string {
  switch (modifier.toLowerCase()) {
    case 'cmdorctrl':
    case 'commandorcontrol':
      return __DARWIN__ ? '⌘' : 'Ctrl'

    case 'ctrl':
    case 'control':
      return __DARWIN__ ? '⌃' : 'Ctrl'

    case 'shift':
      return __DARWIN__ ? '⇧' : 'Shift'
    case 'alt':
      return __DARWIN__ ? '⌥' : 'Alt'

    // Mac only
    case 'cmd':
    case 'command':
      return '⌘'
    case 'option':
      return '⌥'

    // Special case space because no one would be able to see it
    case ' ':
      return 'Space'
  }

  // Not a known modifier, likely a normal key
  return modifier
}

/** Show the given menu items in a contextual menu. */
export async function showContextualMenu(items: ReadonlyArray<IMenuItem>) {
  const indices = await showBrowserContextMenu(items)

  if (indices !== null) {
    const menuItem = findSubmenuItem(items, indices)

    if (menuItem !== undefined && menuItem.action !== undefined) {
      menuItem.action()
    }
  }
}

/**
 * Traverse the submenus of the context menu until we find the appropriate index.
 */
function findSubmenuItem(
  currentContextualMenuItems: ReadonlyArray<IMenuItem>,
  indices: ReadonlyArray<number>
): IMenuItem | undefined {
  let foundMenuItem: IMenuItem | undefined = {
    submenu: currentContextualMenuItems,
  }

  for (const index of indices) {
    if (foundMenuItem === undefined || foundMenuItem.submenu === undefined) {
      return undefined
    }

    foundMenuItem = foundMenuItem.submenu[index]
  }

  return foundMenuItem
}
