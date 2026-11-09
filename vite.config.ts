import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import { nodePolyfills } from 'vite-plugin-node-polyfills'
import path from 'path'
import { cpSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'fs'
import frontMatter from 'front-matter'
import { createRequire } from 'module'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const appPackage = JSON.parse(
  readFileSync(path.resolve(__dirname, 'app/package.json'), 'utf8')
) as { version: string; productName: string }
const channel = process.env.RELEASE_CHANNEL || 'development'

// The `url` npm package that vite-plugin-node-polyfills ships. app/src/web/url.ts
// wraps it, so it needs a name the `url` alias below does not capture.
const polyfillRequire = createRequire(
  createRequire(import.meta.url).resolve('vite-plugin-node-polyfills')
)
const legacyUrlPackage = createRequire(
  polyfillRequire.resolve('node-stdlib-browser')
).resolve('url/')

const staticDirName =
  process.platform === 'darwin'
    ? 'darwin'
    : process.platform === 'win32'
    ? 'win32'
    : 'linux'

/**
 * Mirrors copyStaticResources() in script/build.ts for the files the renderer
 * loads by URL (images, stylesheets). Directories under static/ are data the
 * renderer read through `fs`; those are served by the Rust backend instead.
 */
function copyStaticResources(outDir: string): Plugin {
  return {
    name: 'desktop-copy-static',
    apply: 'build',
    closeBundle() {
      const destination = path.join(outDir, 'static')
      mkdirSync(destination, { recursive: true })
      // copyEmoji() in script/build.ts: the database plus the images of
      // GitHub's custom emoji (unicode ones render as text).
      const gemoji = path.resolve(__dirname, 'gemoji')
      cpSync(path.join(gemoji, 'db/emoji.json'), path.join(outDir, 'emoji.json'))
      cpSync(path.join(gemoji, 'images/emoji'), path.join(outDir, 'emoji'), {
        recursive: true,
        filter: source => path.basename(source) !== 'unicode',
      })

      // The gitignore templates offered when creating a repository.
      const gitignores = path.resolve(__dirname, 'app/static/common/gitignore')
      mkdirSync(path.join(destination, 'gitignore'), { recursive: true })
      for (const name of readdirSync(gitignores)) {
        if (name.endsWith('.gitignore')) {
          cpSync(path.join(gitignores, name), path.join(destination, 'gitignore', name))
        }
      }

      // generateLicenseMetadata() in script/build.ts.
      const chooseALicense = path.resolve(__dirname, 'app/static/common/choosealicense.com')
      const licensesDir = path.join(chooseALicense, '_licenses')
      const licenses = readdirSync(licensesDir)
        .map(file => frontMatter<any>(readFileSync(path.join(licensesDir, file), 'utf8')))
        .map(({ attributes, body }) => ({
          name: attributes.nickname || attributes.title,
          featured: attributes.featured || false,
          hidden: attributes.hidden === undefined || attributes.hidden,
          // A trailing newline keeps generated license files from showing
          // "no newline at end of file" in diffs.
          body: `${body.trim()}\n`,
        }))
        .filter(license => !license.hidden)
      writeFileSync(path.join(destination, 'available-licenses.json'), JSON.stringify(licenses))
      writeFileSync(
        path.join(destination, 'LICENSE.choosealicense.md'),
        'GitHub Desktop uses licensing information provided by choosealicense.com.\n\n' +
          'The bundle in available-licenses.json has been generated from a source list provided at ' +
          'https://github.com/github/choosealicense.com, which is made available under the below license:\n\n' +
          '------------\n\n' +
          readFileSync(path.join(chooseALicense, 'LICENSE.md'), 'utf8')
      )

      for (const dir of ['common', staticDirName]) {
        const source = path.resolve(__dirname, 'app/static', dir)
        for (const entry of readdirSync(source, { withFileTypes: true })) {
          if (entry.isFile()) {
            cpSync(path.join(source, entry.name), path.join(destination, entry.name))
          }
        }
      }
    },
  }
}

const outDir = path.resolve(__dirname, 'out/vite-dist')

export default defineConfig({
  root: 'app',
  base: './',
  plugins: [
    react(),
    copyStaticResources(outDir),
    nodePolyfills({
      include: ['util','buffer','crypto','assert','events','querystring','timers','string_decoder'],
      exclude: ['fs/promises','stream/promises','stream/consumers','fs','stream'],
      globals: { Buffer: true, global: true, process: true },
      protocolImports: true,
    }),
  ],
  define: {
    __PROCESS_KIND__: JSON.stringify('ui'),
    'process.env.NODE_ENV': JSON.stringify(process.env.NODE_ENV || 'production'),
    global: 'window',
    __APP_VERSION__: JSON.stringify(appPackage.version),
    __APP_NAME__: JSON.stringify(appPackage.productName),
    __RELEASE_CHANNEL__: JSON.stringify(channel),
    __SHA__: JSON.stringify('vite'),
    // Mirrors getReplacements() in app/app-info.ts, which the webpack build
    // injected. The UI is built on the host the Rust shell is compiled on.
    __DARWIN__: process.platform === 'darwin',
    __WIN32__: process.platform === 'win32',
    __LINUX__: process.platform === 'linux',
    __DEV__: channel === 'development',
    __DEV_SECRETS__:
      channel === 'development' || !process.env.DESKTOP_OAUTH_CLIENT_SECRET,
    __OAUTH_CLIENT_ID__: JSON.stringify(
      process.env.DESKTOP_OAUTH_CLIENT_ID || '3a723b10ac5575cc5bb9'
    ),
    __OAUTH_SECRET__: JSON.stringify(
      process.env.DESKTOP_OAUTH_CLIENT_SECRET ||
        '22c34d87789a365981ed921352a7b9a8c3f69d54'
    ),
    __UPDATES_URL__: JSON.stringify(process.env.DESKTOP_E2E_UPDATES_URL ?? ''),
    __ERROR_REPORTING_ENDPOINT__: 'undefined',
    __NON_FATAL_ERROR_REPORTING_ENDPOINT__: 'undefined',
    'process.platform': JSON.stringify(process.platform),
    // The renderer resolved bundled resources next to its script. Under the
    // desktop:// protocol those live at the origin root.
    __dirname: JSON.stringify('/__desktop_app__'),
    __filename: JSON.stringify('/__desktop_app__/index.js'),
    'process.env.TEST_ENV': 'undefined',
  },
  resolve: {
    alias: [
      { find: /^electron$/, replacement: path.resolve(__dirname, 'app/src/web/electron.ts') },
      { find: /^@github\/copilot-sdk$/, replacement: path.resolve(__dirname, 'app/src/web/copilot-sdk.ts') },
      { find: /^desktop-url-impl$/, replacement: legacyUrlPackage },
      { find: /^(node:)?url$/, replacement: path.resolve(__dirname, 'app/src/web/url.ts') },
      { find: /^keytar$/, replacement: path.resolve(__dirname, 'app/src/web/keytar.ts') },
      { find: /^desktop-dugite\/(.+)$/, replacement: path.resolve(__dirname, 'app/node_modules/dugite/build/lib') + '/$1.js' },
      { find: /^dugite$/, replacement: path.resolve(__dirname, 'app/src/web/dugite.ts') },
      { find: /^desktop-notifications$/, replacement: path.resolve(__dirname, 'app/src/web/desktop-notifications.ts') },
      { find: /^desktop-trampoline$/, replacement: path.resolve(__dirname, 'app/src/web/desktop-trampoline.ts') },
      { find: /^windows-argv-parser$/, replacement: path.resolve(__dirname, 'app/src/web/windows-argv-parser.ts') },
      { find: /^fs-admin$/, replacement: path.resolve(__dirname, 'app/src/web/fs-admin.ts') },
      { find: /^app-path$/, replacement: path.resolve(__dirname, 'app/src/web/app-path.ts') },
      { find: /^registry-js$/, replacement: path.resolve(__dirname, 'app/src/web/registry-js.ts') },
      { find: /^electron-window-state$/, replacement: path.resolve(__dirname, 'app/src/web/electron-window-state.ts') },
      { find: /^web-utils$/, replacement: path.resolve(__dirname, 'app/src/web/web-utils.ts') },
      { find: /^child_process$/, replacement: path.resolve(__dirname, 'app/src/web/child_process.ts') },
      { find: /^net$/, replacement: path.resolve(__dirname, 'app/src/web/net.ts') },
      { find: /^module$/, replacement: path.resolve(__dirname, 'app/src/web/module.ts') },
      { find: /^os$/, replacement: path.resolve(__dirname, 'app/src/web/os.ts') },
      { find: /^path$/, replacement: path.resolve(__dirname, 'app/src/web/path.ts') },
      { find: /^async_hooks$/, replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: /^node:async_hooks$/, replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: 'node:module', replacement: path.resolve(__dirname, 'app/src/web/module.ts') },
      { find: /^node:fs$/, replacement: path.resolve(__dirname, 'app/src/web/fs.ts') },
      { find: /^node:fs\/promises$/, replacement: path.resolve(__dirname, 'app/src/web/fs-promises.ts') },
      { find: 'node:os', replacement: path.resolve(__dirname, 'app/src/web/os.ts') },
      { find: 'node:path', replacement: path.resolve(__dirname, 'app/src/web/path.ts') },
      { find: 'node:async_hooks', replacement: path.resolve(__dirname, 'app/src/web/async_hooks.ts') },
      { find: 'node:net', replacement: path.resolve(__dirname, 'app/src/web/net.ts') },
      { find: 'node:stream', replacement: path.resolve(__dirname, 'app/src/web/stream.ts') },
      { find: 'node:stream/promises', replacement: path.resolve(__dirname, 'app/src/web/stream-promises.ts') },
      { find: 'node:child_process', replacement: path.resolve(__dirname, 'app/src/web/child_process.ts') },

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
    outDir,
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
