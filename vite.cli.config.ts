import { builtinModules } from 'node:module'
import path from 'node:path'
import { defineConfig } from 'vite'
import { getChannel } from './script/dist-info'

// The packaged Electron launcher executes out/cli.js as CommonJS with
// ELECTRON_RUN_AS_NODE. Keep Node builtins and __dirname native to that host.
export default defineConfig({
  define: {
    __DEV__: JSON.stringify(getChannel() === 'development'),
  },
  build: {
    target: 'node22',
    outDir: path.resolve(__dirname, 'out'),
    emptyOutDir: false,
    copyPublicDir: false,
    sourcemap: true,
    minify: false,
    lib: {
      entry: path.resolve(__dirname, 'app/src/cli/main.ts'),
      formats: ['cjs'],
      fileName: () => 'cli.js',
    },
    rollupOptions: {
      external: id => id.startsWith('node:') || builtinModules.includes(id),
    },
  },
})
