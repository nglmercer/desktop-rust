/**
 * Minimal electron shim for Vite web build.
 * Provides ipcRenderer, shell, webUtils, etc. via Rust bridge.
 * Rust bridge is injected as window.__desktop (see desktop-app/web/ipc-shim.js).
 * For Termux loopback, window.desktop is bridge over WebSocket.
 */

type Listener = (event: any, ...args: any[]) => void

class ShimIpcRenderer {
  private listeners = new Map<string, Set<Listener>>()
  private nextId = 1
  private pending = new Map<number, { resolve: (v:any)=>void; reject:(e:any)=>void; timer:any }>()

  private bridge() {
    return (window as any).__desktop || (window as any).desktop || (window as any).ipcRenderer
  }

  invoke(channel: string, ...args: any[]) {
    const b = this.bridge()
    if (b?.invoke) return b.invoke(channel, ...args)
    // fallback to direct fetch for /__invoke
    const id = Date.now()
    return Promise.reject(new Error(`[vite-shim] no bridge for invoke(${channel})`))
  }

  send(channel: string, ...args: any[]) {
    const b = this.bridge()
    if (b?.send) return b.send(channel, ...args)
    // fallback
    console.warn('[shim] send no bridge', channel)
  }

  sendSync(channel: string, ...args: any[]) {
    // No sync IPC in webview — mirror wry behavior: warn and return promise
    console.warn(`[shim] sendSync(${channel}) is async under wry`)
    return this.invoke(channel, ...args)
  }

  on(channel: string, listener: Listener) {
    if (!this.listeners.has(channel)) this.listeners.set(channel, new Set())
    this.listeners.get(channel)!.add(listener)
    const b = this.bridge()
    if (b?.on) b.on(channel, listener)
    return this
  }

  once(channel: string, listener: Listener) {
    const wrap: Listener = (event, ...args) => {
      this.removeListener(channel, wrap)
      listener(event, ...args)
    }
    return this.on(channel, wrap)
  }

  removeListener(channel: string, listener: Listener) {
    this.listeners.get(channel)?.delete(listener)
    const b = this.bridge()
    if (b?.removeListener) b.removeListener(channel, listener)
    return this
  }

  removeAllListeners(channel?: string) {
    if (channel) this.listeners.delete(channel)
    else this.listeners.clear()
    const b = this.bridge()
    if (b?.removeAllListeners) b.removeAllListeners(channel)
    return this
  }

  addListener(channel: string, listener: Listener) {
    return this.on(channel, listener)
  }

  _dispatch(channel: string, ...args: any[]) {
    const set = this.listeners.get(channel)
    if (set) set.forEach(fn => { try { fn({ sender: null }, ...args) } catch(e){ console.error(e)} })
  }
}

export const ipcRenderer = new ShimIpcRenderer()

export const shell = {
  beep: () => { try { new AudioContext().createOscillator().start() } catch {} },
  openPath: async (p: string) => { throw new Error(`openPath not available in web: ${p}`) },
  openExternal: async (url: string) => { window.open(url, '_blank', 'noopener'); return true },
  showItemInFolder: async (_p: string) => {},
  trashItem: async (_p: string) => { throw new Error('trashItem via ipc') },
}

export const webUtils = {
  getPathForFile: () => '',
  getFilePath: () => '',
}

export const remote = {
  app: { getVersion: () => '3.6.11-beta1', getName: () => 'GitHub Desktop', getPath: () => '', getAppPath: () => '', getPathForFile: () => '' },
  dialog: {},
  getCurrentWindow: () => ({ isFocused: () => document.hasFocus() }),
  getCurrentWebContents: () => ipcRenderer,
}

export const app = {
  getVersion: () => '3.6.11-beta1',
  getName: () => 'GitHub Desktop',
  getPath: (_name: string) => '',
  getAppPath: () => '',
  getPathForFile: () => '',
  on: (_ev:string,_fn:any)=>{},
  once: (_ev:string,_fn:any)=>{},
}

export const Menu = {
  buildFromTemplate: (template:any[]) => ({
    getMenuItemById: (_id:string)=> null,
    append: (_item:any)=>{},
    popup: (_opts?:any)=>{},
    setApplicationMenu: (_m:any)=>{},
  }),
  setApplicationMenu: (_m:any)=>{},
  getApplicationMenu: () => null,
}
export const MenuItem = class {}
export const ipcMain = {} as any
export const contextBridge = { exposeInMainWorld(_k:string,_v:any){} }
export const nativeTheme = { themeSource: 'system', shouldUseDarkColors: false, on: (_e:string,_f:any)=>{} }
export const screen = { getPrimaryDisplay: () => ({ workAreaSize: { width: 1920, height: 1080 } }), on: (_e:string,_f:any)=>{} }
export type BrowserWindowType = any
// BrowserWindow placeholder is defined above via BrowserWindowShim

export const dialog = {
  showOpenDialog: async () => ({ canceled: true, filePaths: [] as string[] }),
  showSaveDialog: async () => ({ canceled: true, filePath: undefined as string|undefined }),
  showMessageBox: async () => ({ response: 0 }),
  showErrorBox: (_t:string,_c:string)=>{},
}

export const BrowserWindowShim = {
  getFocusedWindow: () => null,
  getAllWindows: () => [],
}
// re-export for compat
export { BrowserWindowShim as BrowserWindow }

export default { ipcRenderer, shell, webUtils, remote, app, dialog, BrowserWindow: BrowserWindowShim }
