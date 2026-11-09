import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { nodePolyfills } from 'vite-plugin-node-polyfills'
import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  root: 'app',
  base: './',
  plugins: [
    react(),
    nodePolyfills({
      include: ['path','os','util','buffer','crypto','assert','events','querystring','url','timers','string_decoder','net','async_hooks'],
      exclude: ['fs/promises','stream/promises','stream/consumers','fs','stream'],
      globals: { Buffer: true, global: true, process: true },
      protocolImports: true,
    }),
  ],
  define: {
    __PROCESS_KIND__: JSON.stringify('ui'),
    'process.env.NODE_ENV': JSON.stringify(process.env.NODE_ENV || 'production'),
    global: 'window',
    __APP_VERSION__: JSON.stringify('3.6.11-beta1'),
    __RELEASE_CHANNEL__: JSON.stringify('development'),
    __SHA__: JSON.stringify('vite'),
  },
  resolve: {
    alias: [
      { find: /^electron$/, replacement: path.resolve(__dirname, 'app/src/web/electron.ts') },
      { find: /^keytar$/, replacement: path.resolve(__dirname, 'app/src/web/keytar.ts') },
      { find: /^dugite$/, replacement: path.resolve(__dirname, 'app/src/web/dugite.ts') },
      { find: /^desktop-notifications$/, replacement: path.resolve(__dirname, 'app/src/web/desktop-notifications.ts') },
      { find: /^desktop-trampoline$/, replacement: path.resolve(__dirname, 'app/src/web/desktop-trampoline.ts') },
      { find: /^windows-argv-parser$/, replacement: path.resolve(__dirname, 'app/src/web/windows-argv-parser.ts') },
      { find: /^fs-admin$/, replacement: path.resolve(__dirname, 'app/src/web/fs-admin.ts') },
      { find: /^app_path$/, replacement: path.resolve(__dirname, 'app/src/web/app-path.ts') },
      { find: /^registry-js$/, replacement: path.resolve(__dirname, 'app/src/web/registry-js.ts') },
      { find: /^electron-window-state$/, replacement: path.resolve(__dirname, 'app/src/web/electron-window-state.ts') },
      { find: /^web-utils$/, replacement: path.resolve(__dirname, 'app/src/web/web-utils.ts') },
      { find: /^child_process$/, replacement: path.resolve(__dirname, 'app/src/web/child_process.ts') },
      { find: /^net$/, replacement: path.resolve(__dirname, 'app/src/web/net.ts') },
      { find: /^module$/, replacement: path.resolve(__dirname, 'app/src/web/module.ts') },
      { find: /^os$/, replacement: path.resolve(__dirname, 'app/src/web/os.ts') },
      { find: /^path$/, replacement: 'path-browserify' },
      { find: /^async_hooks$/, replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: /^node:async_hooks$/, replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: 'node:module', replacement: path.resolve(__dirname, 'app/src/web/module.ts') },
      { find: /^node:fs$/, replacement: path.resolve(__dirname, 'app/src/web/fs.ts') },
      { find: /^node:fs\/promises$/, replacement: path.resolve(__dirname, 'app/src/web/fs-promises.ts') },
      { find: 'node:os', replacement: path.resolve(__dirname, 'app/src/web/os.ts') },
      { find: 'node:path', replacement: 'path-browserify' },
      { find: 'node:async_hooks', replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: 'node:net', replacement: path.resolve(__dirname, 'app/src/web/net.ts') },
      { find: 'node:stream', replacement: path.resolve(__dirname, 'app/src/web/stream.ts') },
      { find: 'node:stream/promises', replacement: path.resolve(__dirname, 'app/src/web/stream-promises.ts') },
      { find: 'node:crypto', replacement: path.resolve(__dirname, 'app/src/web/crypto.ts') },
      { find: 'node:util', replacement: path.resolve(__dirname, 'app/src/web/util.ts') },
      { find: 'node:buffer', replacement: path.resolve(__dirname, 'app/src/web/buffer.ts') },
      { find: 'node:assert', replacement: path.resolve(__dirname, 'app/src/web/assert.ts') },
      { find: 'node:querystring', replacement: path.resolve(__dirname, 'app/src/web/querystring.ts') },
      { find: 'node:child_process', replacement: path.resolve(__dirname, 'app/src/web/child_process.ts') },
      { find: 'node:timers', replacement: path.resolve(__dirname, 'app/src/web/timers.ts') },

      { find: /^fs$/u, replacement: path.resolve(__dirname, 'app/src/web/fs.ts') },
      { find: /^fs\/promises$/, replacement: path.resolve(__dirname, 'app/src/web/fs-promises.ts') },
      { find: /^stream\/promises$/, replacement: path.resolve(__dirname, 'app/src/web/stream-promises.ts') },
      { find: /^stream$/u, replacement: path.resolve(__dirname, 'app/src/web/stream.ts') },
    ],
    extensions: ['.ts', '.tsx', '.js', '.jsx'],
  },
  css: {
    preprocessorOptions: {
      scss: {
        includePaths: [
          path.resolve(__dirname, 'app/styles'),
          path.resolve(__dirname, 'node_modules/.pnpm/primer-support@4.7.2/node_modules/primer-support'),
          path.resolve(__dirname, 'node_modules'),
        ],
      },
    },
  },
  build: {
    outDir: path.resolve(__dirname, 'out/vite-dist'),
    emptyOutDir: true,
    sourcemap: true,
    chunkSizeWarningLimit: 2000,
    commonjsOptions: { transformMixedEsModules: true },
    rollupOptions: {
      input: path.resolve(__dirname, 'app/index.html'),
      onwarn: (warning, warn) => {
        if (warning.code === 'MISSING_EXPORT') return
        if (warning.message.includes('Module "')) return
        warn(warning)
      },
    },
  },
  server: { host:'127.0.0.1', port:5173, strictPort:true },
  preview: { host:'127.0.0.1', port:4173 },
})
