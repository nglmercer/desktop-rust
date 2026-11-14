import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import {
  requireAbsentViteOutputFiles,
  viteOutputDirectory,
} from './script/vite-output'

const root = path.dirname(fileURLToPath(import.meta.url))

const outputDirectory = viteOutputDirectory(root)
if (process.env.DESKTOP_VITE_DIST !== undefined) {
  requireAbsentViteOutputFiles(outputDirectory, [
    'ipc-shim.js',
    'ipc-shim.js.map',
  ])
}

// Wry injects this classic script before the renderer loads. It must remain
// a single standalone file, with no module imports or shared chunks.
export default defineConfig({
  root,
  logLevel: 'warn',
  build: {
    outDir: outputDirectory,
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
