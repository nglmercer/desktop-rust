import { builtinModules, createRequire } from 'node:module'
import path from 'node:path'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import { getReplacements } from './app/app-info'
import { packagedExternals } from './app/build-externals'
import { getDistArchitecture } from './script/dist-info'

if (process.platform !== 'linux') {
  throw new Error(
    'The experimental renderer build currently supports Linux only'
  )
}

const root = __dirname
const appRequire = createRequire(path.resolve(root, 'app/package.json'))
const native = [
  {
    name: 'fs_admin',
    loader: appRequire.resolve('fs-admin'),
    pattern: './build/Release/fs_admin.node',
  },
  {
    name: 'keytar',
    loader: appRequire.resolve('keytar'),
    pattern: '../build/Release/keytar.node',
  },
] as const

function nativeAddons(): Plugin {
  return {
    name: 'electron-renderer-native-addons',
    enforce: 'pre',
    buildStart() {
      for (const addon of native) {
        const file =
          process.env[`DESKTOP_RENDERER_${addon.name.toUpperCase()}_NODE`] ??
          path.resolve(path.dirname(addon.loader), addon.pattern)
        if (!existsSync(file)) {
          this.error(`Missing ${addon.name} addon: ${file}`)
        }
        this.emitFile({
          type: 'asset',
          fileName: `${addon.name}.node`,
          source: readFileSync(file),
        })
      }
    },
    transform(code, id) {
      const addon = native.find(item => item.loader === id)
      if (!addon) {
        return null
      }
      const replacement = `desktop-native-addon:${addon.name}`
      if (!code.includes(addon.pattern)) {
        this.error(`Unexpected ${addon.name} native loader`)
      }
      return { code: code.replace(addon.pattern, replacement), map: null }
    },
  }
}
const sdk = appRequire.resolve('@github/copilot-sdk')
const koffiPath = appRequire.resolve('koffi', { paths: [sdk] })
const koffiName = `@koromix/koffi-${process.platform}-${getDistArchitecture()}`
const koffiEntry = appRequire.resolve(koffiName, { paths: [koffiPath] })
function koffiPackage(): Plugin {
  const sourceRoot = path.dirname(koffiEntry)
  return {
    name: 'electron-renderer-koffi',
    enforce: 'pre',
    buildStart() {
      const emit = (dir: string) => {
        for (const entry of readdirSync(dir, { withFileTypes: true })) {
          const file = path.join(dir, entry.name)
          if (entry.isDirectory()) {
            emit(file)
          } else if (entry.isFile()) {
            this.emitFile({
              type: 'asset',
              fileName:
                'native/koffi/' +
                path.relative(sourceRoot, file).split(path.sep).join('/'),
              source: readFileSync(file),
            })
          } else {
            this.error(`Unsupported link in Koffi package: ${file}`)
          }
        }
      }
      emit(sourceRoot)
    },
    transform(code, id) {
      if (
        id !== path.resolve(path.dirname(koffiPath), 'src/koffi/src/static.js')
      ) {
        return null
      }
      const needle = `'${koffiName}'`
      if (!code.includes(needle)) {
        this.error('Koffi loader changed')
      }
      return {
        code: code.replace(needle, `'./native/koffi/index.js'`),
        map: null,
      }
    },
  }
}
const replacements = getReplacements()
function rendererHtml(): Plugin {
  return {
    name: 'electron-renderer-html',
    generateBundle() {
      const template = readFileSync(
        path.resolve(root, 'app/static/index.html'),
        'utf8'
      )
      this.emitFile({
        type: 'asset',
        fileName: 'index.html',
        source: template
          .replace(
            '</head>',
            '<link rel="stylesheet" href="./renderer.css" /></head>'
          )
          .replace('</body>', '<script src="./renderer.js"></script></body>'),
      })
    },
  }
}
export default defineConfig({
  root: 'app',
  plugins: [react(), nativeAddons(), koffiPackage(), rendererHtml()],
  define: { ...replacements, __PROCESS_KIND__: JSON.stringify('ui') },
  resolve: {
    mainFields: ['module', 'main'],
    conditions: ['node'],
    alias: [
      {
        find: /^@desktop-assets\//,
        replacement: path.resolve(root, 'app/static/common') + '/',
      },
      {
        find: /^react\/jsx-runtime$/,
        replacement: appRequire.resolve('react/jsx-runtime'),
      },
      {
        find: /^desktop-notifications$/,
        replacement: path.resolve(
          root,
          'vendor/desktop-notifications/lib/index.ts'
        ),
      },
      {
        find: /^desktop-notifications\/dist\/(.+)$/,
        replacement:
          path.resolve(root, 'vendor/desktop-notifications/lib') + '/$1.ts',
      },
      {
        find: /^windows-argv-parser$/,
        replacement: path.resolve(root, 'vendor/windows-argv-parser/index.ts'),
      },
      {
        find: /^vscode-jsonrpc(?:\/node(?:\.js)?)?$/,
        replacement: createRequire(sdk).resolve(
          'vscode-jsonrpc/lib/node/main.js'
        ),
      },
      { find: koffiName, replacement: koffiEntry },
    ],
  },
  css: {
    preprocessorOptions: {
      scss: {
        loadPaths: [
          path.resolve(root, 'app/styles'),
          path.resolve(root, 'app/node_modules'),
          path.resolve(root, 'node_modules'),
        ],
      },
    },
  },
  build: {
    target: 'node22',
    outDir: path.resolve(root, 'node_modules/.cache/desktop-renderer-vite'),
    emptyOutDir: true,
    copyPublicDir: false,
    sourcemap: true,
    minify: false,
    lib: {
      entry: path.resolve(root, 'app/src/ui/index.tsx'),
      formats: ['cjs'],
      fileName: () => 'renderer.js',
      cssFileName: 'renderer',
    },
    commonjsOptions: { strictRequires: true },
    rollupOptions: {
      external: id =>
        id === 'electron' ||
        packagedExternals.includes(id) ||
        id.startsWith('node:') ||
        id.startsWith('desktop-native-addon:') ||
        builtinModules.includes(id),
      output: {
        paths: id =>
          id.startsWith('desktop-native-addon:')
            ? `./${id.slice('desktop-native-addon:'.length)}.node`
            : id,
      },
    },
  },
})
