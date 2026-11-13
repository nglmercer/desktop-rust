import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'

const root = path.dirname(fileURLToPath(import.meta.url))

// Wry injects this classic script before the renderer loads. It must remain
// a single standalone file, with no module imports or shared chunks.
export default defineConfig({
  root,
  logLevel: 'warn',
  build: {
    outDir: path.join(root, 'out/vite-dist'),
    emptyOutDir: false,
    lib: {
      entry: path.join(root, 'rust/crates/desktop-app/web/ipc-shim.ts'),
      name: 'DesktopIpcBootstrap',
      formats: ['iife'],
      fileName: () => 'ipc-shim.js',
    },
    rollupOptions: { output: { inlineDynamicImports: true } },
  },
})
