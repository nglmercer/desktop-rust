import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { normalizePath, type Plugin } from 'vite'

export interface IRendererNativeAddon {
  readonly name: string
  readonly loader: string
  readonly pattern: string
}

/** Match Rollup IDs to filesystem paths on both Windows and Unix hosts. */
export function rendererModuleId(
  id: string,
  platform: NodeJS.Platform = process.platform
): string {
  return normalizePath(platform === 'win32' ? id.replace(/\\/g, '/') : id)
}

/** Keep required native modules beside the relocatable renderer bundle. */
export function rendererNativeAddons(
  addons: ReadonlyArray<IRendererNativeAddon>,
  binaryOverrides: Readonly<Record<string, string | undefined>> = {}
): Plugin {
  return {
    name: 'electron-renderer-native-addons',
    enforce: 'pre',
    buildStart() {
      for (const addon of addons) {
        const file =
          binaryOverrides[addon.name] ??
          path.resolve(path.dirname(addon.loader), addon.pattern)
        if (!existsSync(file)) {
          this.error(`Missing ${addon.name} addon: ${file}`)
        }
        this.addWatchFile(file)
        this.emitFile({
          type: 'asset',
          fileName: `${addon.name}.node`,
          source: readFileSync(file),
        })
      }
    },
    transform(code, id) {
      const addon = addons.find(
        item => rendererModuleId(item.loader) === rendererModuleId(id)
      )
      if (addon === undefined) return null
      const parts = code.split(`require('${addon.pattern}')`)
      if (parts.length !== 2) {
        this.error(`Unexpected ${addon.name} native loader`)
      }
      return {
        code: parts.join(`require('desktop-native-addon:${addon.name}')`),
        map: null,
      }
    },
    buildEnd(error) {
      if (error) return
      const modules = new Set(
        Array.from(this.getModuleIds())
          .filter(id => this.getModuleInfo(id)?.isExternal === false)
          .map(id => rendererModuleId(id))
      )
      for (const addon of addons) {
        if (!modules.has(rendererModuleId(addon.loader))) {
          this.error(`Native loader ${addon.name} was not processed`)
        }
      }
    },
  }
}
