import { existsSync, readFileSync } from 'node:fs'
import { builtinModules, createRequire } from 'node:module'
import path from 'node:path'
import { defineConfig, normalizePath, type Plugin } from 'vite'
import { getReplacements } from './app/app-info'
import { packagedExternals } from './app/build-externals'

const appRequire = createRequire(path.resolve(__dirname, 'app/package.json'))

/** Compare Vite's normalized IDs with paths returned by Node on each platform. */
export function moduleId(
  id: string,
  platform: NodeJS.Platform = process.platform
) {
  return normalizePath(platform === 'win32' ? id.replace(/\\/g, '/') : id)
}

const nativeModules = [
  {
    name: 'desktop-notifications',
    source: appRequire.resolve('desktop-notifications/dist/native-module.js'),
    original: '../build/Release/desktop-notifications.node',
    platforms: ['darwin', 'win32'],
  },
  {
    name: 'registry',
    source: appRequire.resolve('registry-js/dist/lib/registry.js'),
    original: '../../build/Release/registry.node',
    platforms: ['win32'],
  },
] as const

/** Preserve the existing native load sites and emit addons beside main.js. */
export function mainNativeModules(
  platform: NodeJS.Platform = process.platform,
  binaryOverrides: Readonly<Record<string, string>> = {}
): Plugin {
  const transformed = new Set<string>()
  return {
    name: 'desktop-main-native-modules',
    enforce: 'pre',
    buildStart() {
      for (const module of nativeModules) {
        if (!module.platforms.some(value => value === platform)) continue
        const binary =
          binaryOverrides[module.name] ??
          path.resolve(path.dirname(module.source), module.original)
        if (!existsSync(binary)) {
          this.error(`Missing required native module ${module.name}: ${binary}`)
        }
        this.addWatchFile(binary)
        this.emitFile({
          type: 'asset',
          fileName: `${module.name}.node`,
          source: readFileSync(binary),
        })
      }
    },
    transform(code, id) {
      const module = nativeModules.find(
        value => moduleId(value.source) === moduleId(id)
      )
      if (module === undefined) return null
      const parts = code.split(`require('${module.original}')`)
      if (parts.length !== 2)
        this.error(`Unexpected ${module.name} native loader`)
      transformed.add(module.name)
      return { code: parts.join(`require('./${module.name}.node')`), map: null }
    },
    buildEnd(error) {
      if (error) return
      for (const module of nativeModules) {
        if (
          module.platforms.some(value => value === platform) &&
          !transformed.has(module.name)
        ) {
          this.error(`Native loader ${module.name} was not processed`)
        }
      }
    },
  }
}

export const nativeRequires = nativeModules.map(
  module => `./${module.name}.node`
)

/** Bundle the development extension installer used by the main process. */
export function bundleDevtools(): Plugin {
  const entry = moduleId(
    path.resolve(__dirname, 'app/src/main-process/main.ts')
  )
  const virtualId = 'virtual:desktop-devtools-loader'
  const loader = moduleId(
    path.resolve(__dirname, 'node_modules/.desktop-vite/devtools-loader.cjs')
  )
  const installer = appRequire.resolve('electron-devtools-installer')
  return {
    name: 'desktop-main-devtools',
    enforce: 'pre',
    resolveId(id) {
      if (id === virtualId) return loader
      return null
    },
    load(id) {
      if (moduleId(id) === loader)
        return `module.exports = () => require(${JSON.stringify(installer)})`
      return null
    },
    transform(code, id) {
      if (moduleId(id) !== entry) return null
      const parts = code.split("require('electron-devtools-installer')")
      if (parts.length !== 2) this.error('Unexpected Electron devtools loader')
      return {
        code: `import __desktopLoadDevtools from '${virtualId}'\n${parts.join(
          '__desktopLoadDevtools()'
        )}`,
        map: null,
      }
    },
  }
}

const replacements = getReplacements()

export default defineConfig({
  plugins: [
    mainNativeModules(),
    ...(replacements.__DEV__ ? [bundleDevtools()] : []),
  ],
  define: { ...replacements, __PROCESS_KIND__: JSON.stringify('main') },
  resolve: { mainFields: ['module', 'main'], conditions: ['node'] },
  build: {
    target: 'node22',
    outDir: path.resolve(__dirname, 'out'),
    emptyOutDir: false,
    copyPublicDir: false,
    sourcemap: true,
    minify: process.env.NODE_ENV === 'production' ? 'esbuild' : false,
    lib: {
      entry: path.resolve(__dirname, 'app/src/main-process/main.ts'),
      formats: ['cjs'],
      fileName: () => 'main.js',
    },
    commonjsOptions: { ignore: nativeRequires, strictRequires: true },
    rollupOptions: {
      external: id =>
        id === 'electron' ||
        packagedExternals.includes(id) ||
        id.startsWith('node:') ||
        builtinModules.includes(id),
    },
  },
})
