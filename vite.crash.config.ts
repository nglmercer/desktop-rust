import { builtinModules } from 'node:module'
import path from 'node:path'
import { defineConfig, type Plugin } from 'vite'
import { getReplacements } from './app/app-info'

/** Keep the crash window usable from file:// beside the packaged bundles. */
function crashHtml(): Plugin {
  return {
    name: 'desktop-crash-html',
    generateBundle() {
      this.emitFile({
        type: 'asset',
        fileName: 'crash.html',
        source:
          '<!doctype html><html><head><meta charset="utf-8"><title>GitHub Desktop</title><link rel="stylesheet" href="./crash.css"></head><body><script src="./crash.js"></script></body></html>',
      })
    },
  }
}

export default defineConfig({
  plugins: [crashHtml()],
  define: { ...getReplacements(), __PROCESS_KIND__: JSON.stringify('crash') },
  resolve: { mainFields: ['module', 'main'], conditions: ['node'] },
  build: {
    target: 'chrome130',
    outDir: path.resolve(__dirname, 'out'),
    emptyOutDir: false,
    copyPublicDir: false,
    sourcemap: true,
    minify: process.env.NODE_ENV === 'production' ? 'esbuild' : false,
    lib: {
      entry: path.resolve(__dirname, 'app/src/crash/index.tsx'),
      formats: ['cjs'],
      fileName: () => 'crash.js',
      cssFileName: 'crash',
    },
    rollupOptions: {
      external: id =>
        id === 'electron' ||
        id.startsWith('node:') ||
        builtinModules.includes(id),
    },
  },
})
