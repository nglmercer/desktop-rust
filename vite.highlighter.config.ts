import { defineConfig, type Plugin } from 'vite'
import path from 'path'
import { createRequire } from 'module'
import { fileURLToPath } from 'url'
import {
  requireAbsentViteOutputFiles,
  viteOutputDirectory,
} from './script/vite-output'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const outputDirectory = viteOutputDirectory(__dirname)
if (process.env.DESKTOP_VITE_DIST !== undefined) {
  requireAbsentViteOutputFiles(outputDirectory, [
    'highlighter.js',
    'highlighter.js.map',
  ])
}

const runmodeEntry = createRequire(
  path.resolve(__dirname, 'app/package.json')
).resolve('codemirror/addon/runmode/runmode.node.js')

/**
 * CodeMirror's node runmode attaches itself to `codemirror` through Node's
 * `require.cache`, and its mode files ask for the full editor library, which
 * needs the DOM. In the worker, every request for `lib/codemirror` is answered
 * by the runmode module instead, and the cache lines are dropped. This is what
 * the webpack build's `common.highlighter` config did.
 */
const codeMirrorNodeRunmode: Plugin = {
  name: 'highlighter-codemirror-runmode',
  enforce: 'pre',
  resolveId(source) {
    if (
      source === 'codemirror' ||
      /(^|\/)lib\/codemirror(\.js)?$/.test(source)
    ) {
      return runmodeEntry
    }
    return null
  },
  transform(code, id) {
    if (!/runmode\.node\.js$/.test(id)) {
      return null
    }
    // Keep line numbers stable when removing Node's cache wiring so the
    // worker's source map can still identify the original runmode lines.
    const transformed = code
      .replace(/^require\.cache\[.*$/gm, '')
      .replace(/require\("\.\.\/\.\.\/lib\/codemirror"\)/g, 'exports')
    return {
      code: transformed,
      map: {
        version: 3,
        sources: [id],
        sourcesContent: [code],
        names: [],
        mappings: transformed
          .split('\n')
          .map((_, index) => (index === 0 ? 'AAAA' : 'AACA'))
          .join(';'),
      },
    }
  },
}

/**
 * The syntax highlighter runs in a classic Web Worker that the renderer loads
 * as `highlighter.js` next to the bundle. It is built on its own so the
 * output is one self-contained script (a worker cannot load split chunks).
 */
export default defineConfig({
  root: 'app',
  plugins: [codeMirrorNodeRunmode],
  logLevel: 'warn',
  define: {
    __PROCESS_KIND__: JSON.stringify('highlighter'),
    'process.env.NODE_ENV': JSON.stringify('production'),
    __SHA__: JSON.stringify('vite'),
  },
  resolve: {
    alias: [
      { find: /^node:path$/, replacement: 'path-browserify' },
      { find: /^path$/, replacement: 'path-browserify' },
    ],
  },
  build: {
    outDir: outputDirectory,
    emptyOutDir: false,
    lib: {
      entry: path.resolve(__dirname, 'app/src/highlighter/index.ts'),
      name: 'highlighter',
      formats: ['iife'],
      fileName: () => 'highlighter.js',
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
  },
})
