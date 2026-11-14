// Types shared with the desktop host over the IPC bridge. They describe the
// payloads the Rust shell exchanges with the renderer.

export interface HostCertificate {
  readonly data: string
  readonly issuerName: string
  readonly subjectName: string
  readonly serialNumber: string
  readonly validStart: number
  readonly validExpiry: number
  readonly fingerprint: string
  readonly issuer?: unknown
  readonly subject?: unknown
  readonly issuerCert?: HostCertificate
}

export interface HostFileFilter {
  name: string
  extensions: string[]
}

export interface HostOpenDialogOptions {
  title?: string
  defaultPath?: string
  buttonLabel?: string
  filters?: HostFileFilter[]
  properties?: Array<
    | 'openFile'
    | 'openDirectory'
    | 'multiSelections'
    | 'showHiddenFiles'
    | 'createDirectory'
    | 'promptToCreate'
    | 'noResolveAliases'
    | 'treatPackageAsDirectory'
    | 'dontAddToRecent'
  >
  message?: string
}

export interface HostSaveDialogOptions {
  title?: string
  defaultPath?: string
  buttonLabel?: string
  filters?: HostFileFilter[]
  message?: string
  nameFieldLabel?: string
  showsTagField?: boolean
  properties?: Array<
    | 'showHiddenFiles'
    | 'createDirectory'
    | 'treatPackageAsDirectory'
    | 'showOverwriteConfirmation'
    | 'dontAddToRecent'
  >
}

export interface HostMenuItemOptions {
  click?: (...args: any[]) => void
  role?: string
  type?: 'normal' | 'separator' | 'submenu' | 'checkbox' | 'radio'
  label?: string
  sublabel?: string
  accelerator?: string
  enabled?: boolean
  visible?: boolean
  checked?: boolean
  submenu?: HostMenuItemOptions[] | HostMenu
  id?: string
  [key: string]: unknown
}

export interface HostMenuItem {
  readonly id: string
  readonly label: string
  readonly accelerator?: string | null
  readonly role?: string
  readonly submenu?: HostMenu
  readonly enabled: boolean
  readonly visible: boolean
  readonly checked: boolean
  readonly type: string
  [key: string]: any
}

export interface HostMenu {
  readonly items: HostMenuItem[]
}

/** The event passed to IPC listeners. */
export interface HostIpcEvent {
  readonly sender?: unknown
  readonly senderId?: number
}

/** What the host reports for the user's title-bar double-click preference. */
export type DoubleClickAction = 'Maximize' | 'Minimize' | 'None'
