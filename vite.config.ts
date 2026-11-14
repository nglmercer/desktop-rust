import { defineConfig, type Plugin } from 'vite'
import { requireEmptyViteOutput, viteOutputDirectory } from './script/vite-output'
import react from '@vitejs/plugin-react'
import { nodePolyfills } from 'vite-plugin-node-polyfills'
import path from 'path'
import { closeSync, existsSync, fchmodSync, fstatSync, lstatSync, mkdirSync, openSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'fs'
import { randomUUID } from 'node:crypto'
import frontMatter from 'front-matter'
import { createRequire } from 'module'
import { realpathSync } from 'fs'
import { fileURLToPath } from 'url'
import { createLicenseDump } from './script/licenses/update-license-dump'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const appPackage = JSON.parse(
  readFileSync(path.resolve(__dirname, 'app/package.json'), 'utf8')
) as { version: string; productName: string }
const channel = process.env.RELEASE_CHANNEL || 'development'

/** Resolve the SDK's Node transport using its own dependency lookup. */
export function resolveCopilotJsonRpc(appDirectory: string): string {
  const appRequire = createRequire(path.join(appDirectory, 'package.json'))
  const sdkRequire = createRequire(appRequire.resolve('@github/copilot-sdk'))
  return sdkRequire.resolve('vscode-jsonrpc/lib/node/main.js')
}

/** Sass imports resolve from the application regardless of package manager. */
export function sassLoadPaths(root: string): string[] {
  return [
    path.join(root, 'app/styles'),
    path.join(root, 'app/node_modules'),
    path.join(root, 'node_modules'),
  ]
}

// The `url` npm package that vite-plugin-node-polyfills ships. app/src/web/url.ts
// wraps it, so it needs a name the `url` alias below does not capture.
const polyfillRequire = createRequire(
  createRequire(import.meta.url).resolve('vite-plugin-node-polyfills')
)
const legacyUrlPackage = createRequire(
  polyfillRequire.resolve('node-stdlib-browser')
).resolve('url/')
// The crypto implementation behind app/src/web/crypto.ts (randomUUID is added there).
const legacyCryptoPackage = createRequire(
  polyfillRequire.resolve('node-stdlib-browser')
).resolve('crypto-browserify')
const legacyStreamPackage = createRequire(
  polyfillRequire.resolve('node-stdlib-browser')
).resolve('stream-browserify')

const staticDirName =
  process.platform === 'darwin'
    ? 'darwin'
    : process.platform === 'win32'
    ? 'win32'
    : 'linux'

function isInside(root: string, target: string): boolean {
  const relative = path.relative(root, target)
  return relative === '' || (relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative))
}

/** Create output directories without following links beneath the trusted root. */
function ensureOutputDirectory(trustedRoot: string, destination: string) {
  const root = path.resolve(trustedRoot)
  const directory = path.resolve(destination)
  if (!isInside(root, directory)) throw new Error('Output directory escapes its trusted root')
  let current = root
  for (const part of path.relative(root, directory).split(path.sep).filter(Boolean)) {
    current = path.join(current, part)
    try {
      const stat = lstatSync(current)
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error(`Unsafe output directory: ${current}`)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      mkdirSync(current)
    }
  }
}

/** Publish a file without opening any pre-existing output symlink. */
function publishOutputFile(destination: string, contents: string | Buffer, mode?: number) {
  const temporary = path.join(path.dirname(destination), `.desktop-${randomUUID()}.tmp`)
  const handle = openSync(temporary, 'wx')
  let closed = false
  try {
    writeFileSync(handle, contents)
    if (mode !== undefined && process.platform !== 'win32') fchmodSync(handle, mode & 0o777)
    closeSync(handle)
    closed = true
    renameSync(temporary, destination)
  } finally {
    if (!closed) closeSync(handle)
    rmSync(temporary, { force: true })
  }
}

function copyOutputFile(source: string, destination: string) {
  const handle = openSync(source, 'r')
  try {
    const metadata = fstatSync(handle)
    if (!metadata.isFile()) throw new Error(`Unsupported bundled resource: ${source}`)
    publishOutputFile(destination, readFileSync(handle), metadata.mode)
  } finally {
    closeSync(handle)
  }
}

function writeOutputFile(destination: string, contents: string) {
  publishOutputFile(destination, contents)
}

/** Copy bundled emoji while rejecting links and excluding Unicode images. */
function copyEmojiImages(source: string, destination: string, output: string, sources?: Set<string>) {
  ensureOutputDirectory(output, destination)
  for (const entry of readdirSync(source, { withFileTypes: true })) {
    if (entry.name === 'unicode') continue
    const from = path.join(source, entry.name)
    const to = path.join(destination, entry.name)
    if (entry.isDirectory()) copyEmojiImages(from, to, output, sources)
    else if (entry.isFile()) {
      copyOutputFile(from, to)
      sources?.add(from)
    }
    else throw new Error(`Unsupported bundled emoji resource: ${from}`)
  }
}

/** Materialize template symlinks without following existing output links. */
export function copyGitignoreTemplates(source: string, destination: string, trustedRoot = path.dirname(destination), sources?: Set<string>) {
  const root = realpathSync(source)
  const anchor = path.resolve(trustedRoot)
  const output = path.resolve(destination)
  if (!isInside(anchor, output)) throw new Error('Output directory escapes its trusted root')
  const canonicalOutput = path.resolve(realpathSync(anchor), path.relative(anchor, output))
  if (isInside(root, canonicalOutput) || isInside(canonicalOutput, root)) {
    throw new Error('Gitignore source and output directories overlap')
  }
  ensureOutputDirectory(anchor, output)
  for (const name of readdirSync(source)) {
    if (!name.endsWith('.gitignore')) continue
    const resolved = realpathSync(path.join(source, name))
    const relative = path.relative(root, resolved)
    if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new Error(`Gitignore template escapes its source directory: ${name}`)
    }
    // A previous build may have left an absolute symlink in the output. Copy
    // into a new file and replace the directory entry so it is never followed.
    copyOutputFile(resolved, path.join(destination, name))
    sources?.add(path.join(source, name))
  }
}

/**
 * The git submodules the renderer's static resources come from. Without them
 * the bundle would silently miss emoji and gitignore templates, so the build
 * stops here with the command that fixes it.
 */
function requireSubmodules(): Plugin {
  return {
    name: 'desktop-require-submodules',
    apply: 'build',
    buildStart() {
      const missing = [
        'gemoji/db/emoji.json',
        'app/static/common/gitignore',
      ].filter(file => !existsSync(path.resolve(__dirname, file)))
      if (missing.length > 0) {
        this.error(
          `Missing git submodule content: ${missing.join(', ')}. ` +
            'Run `git submodule update --init --recursive` from the repository root.'
        )
      }
    },
  }
}

/**
 * Source maps are for diagnosis. The bundle is embedded in the Rust binary, so
 * maps are written beside it only when DESKTOP_VITE_SOURCEMAP is set, and then
 * they are moved out to out/vite-sourcemaps.
 */
function outputRoot(outDir: string): string {
  return isInside(__dirname, outDir) ? __dirname : path.dirname(outDir)
}

export function moveSourceMaps(): Plugin {
  let outDir: string
  return {
    name: 'desktop-move-sourcemaps',
    apply: 'build',
    configResolved(config) {
      outDir = path.resolve(config.root, config.build.outDir)
    },
    writeBundle() {
      if (!process.env.DESKTOP_VITE_SOURCEMAP) {
        return
      }
      const mapsDir = path.resolve(outDir, '..', 'vite-sourcemaps')
      const trustedRoot = outputRoot(outDir)
      ensureOutputDirectory(trustedRoot, outDir)
      const walk = (directory: string) => {
        for (const entry of readdirSync(directory, { withFileTypes: true })) {
          const file = path.join(directory, entry.name)
          if (entry.isDirectory()) {
            walk(file)
          } else if (entry.name.endsWith('.map')) {
            const target = path.join(mapsDir, path.relative(outDir, file))
            ensureOutputDirectory(trustedRoot, path.dirname(target))
            renameSync(file, target)
          }
        }
      }
      walk(outDir)
    },
  }
}

/**
 * Mirrors copyStaticResources() in script/build.ts for the files the renderer
 * loads by URL (images, stylesheets). Directories under static/ are data the
 * renderer read through `fs`; those are served by the Rust backend instead.
 */
export function copyStaticResources(
  platformDirectory = staticDirName,
  projectRoot = __dirname,
  licenseDump = createLicenseDump
): Plugin {
  let outDir: string
  let copiedSources = new Set<string>()
  return {
    name: 'desktop-copy-static',
    apply: 'build',
    configResolved(config) {
      outDir = path.resolve(config.root, config.build.outDir)
    },
    buildStart() {
      for (const source of copiedSources) {
        if (!existsSync(source)) {
          this.error(`A copied static resource was removed: ${source}. Stop the development launcher, run pnpm vite:build, then restart it to clear the old output.`)
        }
      }
      for (const input of ['LICENSE', 'gemoji/db/emoji.json', 'app/static/common/choosealicense.com/LICENSE.md']) {
        const file = path.resolve(projectRoot, input)
        if (existsSync(file)) this.addWatchFile(file)
      }
      const watchDirectory = (directory: string, recursive: boolean, links = false) => {
        if (!existsSync(directory)) return
        this.addWatchFile(directory)
        for (const entry of readdirSync(directory, { withFileTypes: true })) {
          if (recursive && entry.name === 'unicode') continue
          const file = path.join(directory, entry.name)
          if (entry.isFile()) {
            this.addWatchFile(file)
          } else if (entry.isDirectory() && recursive) {
            watchDirectory(file, true)
          } else if (entry.isSymbolicLink() && links) {
            this.addWatchFile(file)
            try {
              const resolved = realpathSync(file)
              if (isInside(realpathSync(directory), resolved)) this.addWatchFile(resolved)
            } catch {
              // Copying reports a broken template link as a build error.
            }
          }
        }
      }
      watchDirectory(path.resolve(projectRoot, 'gemoji/images/emoji'), true)
      watchDirectory(path.resolve(projectRoot, 'app/static/common'), false)
      watchDirectory(path.resolve(projectRoot, 'app/static/common/gitignore'), false, true)
      watchDirectory(path.resolve(projectRoot, 'app/static/common/choosealicense.com/_licenses'), false)
      watchDirectory(path.resolve(projectRoot, `app/static/${platformDirectory}`), false)
    },
    async writeBundle() {
      const currentSources = new Set<string>()
      const trustedRoot = outputRoot(outDir)
      ensureOutputDirectory(trustedRoot, outDir)
      const output = realpathSync(outDir)
      const destination = path.join(output, 'static')
      ensureOutputDirectory(output, destination)
      // Keep the acknowledgements dialog available in the standalone web
      // renderer as well as in packaged Electron builds.
      writeOutputFile(
        path.join(destination, 'licenses.json'),
        JSON.stringify(await licenseDump(projectRoot))
      )
      const projectLicense = path.join(projectRoot, 'LICENSE')
      if (existsSync(projectLicense)) currentSources.add(projectLicense)
      // copyEmoji() in script/build.ts: the database plus the images of
      // GitHub's custom emoji (unicode ones render as text).
      const gemoji = path.resolve(projectRoot, 'gemoji')
      const emojiDatabase = path.join(gemoji, 'db/emoji.json')
      copyOutputFile(emojiDatabase, path.join(output, 'emoji.json'))
      currentSources.add(emojiDatabase)
      copyEmojiImages(path.join(gemoji, 'images/emoji'), path.join(output, 'emoji'), output, currentSources)

      // The gitignore templates offered when creating a repository.
      const gitignores = path.resolve(projectRoot, 'app/static/common/gitignore')
      copyGitignoreTemplates(gitignores, path.join(destination, 'gitignore'), output, currentSources)

      // generateLicenseMetadata() in script/build.ts.
      const chooseALicense = path.resolve(projectRoot, 'app/static/common/choosealicense.com')
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
      writeOutputFile(path.join(destination, 'available-licenses.json'), JSON.stringify(licenses))
      writeOutputFile(
        path.join(destination, 'LICENSE.choosealicense.md'),
        'GitHub Desktop uses licensing information provided by choosealicense.com.\n\n' +
          'The bundle in available-licenses.json has been generated from a source list provided at ' +
          'https://github.com/github/choosealicense.com, which is made available under the below license:\n\n' +
          '------------\n\n' +
          readFileSync(path.join(chooseALicense, 'LICENSE.md'), 'utf8')
      )
      currentSources.add(path.join(chooseALicense, 'LICENSE.md'))

      for (const dir of ['common', platformDirectory]) {
        const source = path.resolve(projectRoot, 'app/static', dir)
        for (const entry of readdirSync(source, { withFileTypes: true })) {
          if (entry.isFile()) {
            const resource = path.join(source, entry.name)
            copyOutputFile(resource, path.join(destination, entry.name))
            currentSources.add(resource)
          }
        }
      }
      copiedSources = currentSources
    },
  }
}

const outDir = viteOutputDirectory(__dirname)
if (process.env.DESKTOP_VITE_DIST !== undefined) requireEmptyViteOutput(outDir)

export default defineConfig({
  root: 'app',
  base: './',
  plugins: [
    react(),
    requireSubmodules(),
    copyStaticResources(),
    moveSourceMaps(),
    nodePolyfills({
      include: ['util','buffer','crypto','assert','events','querystring','timers','string_decoder'],
      globals: { Buffer: true, global: true, process: true },
      protocolImports: true,
      // `crypto` is replaced for both import and require paths (the Copilot
      // SDK's CommonJS build reads it with require); see app/src/web/crypto.ts.
      overrides: { crypto: path.resolve(__dirname, 'app/src/web/crypto.ts') },
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
    'process.arch': JSON.stringify(process.arch),
    // The renderer resolved bundled resources next to its script. Under the
    // desktop:// protocol those live at the origin root.
    __dirname: JSON.stringify('/__desktop_app__'),
    __filename: JSON.stringify('/__desktop_app__/index.js'),
    'process.env.TEST_ENV': 'undefined',
  },
  resolve: {
    alias: [
      { find: /^@desktop-assets\//, replacement: path.resolve(__dirname, 'app/static/common') + '/' },
      { find: /^electron$/, replacement: path.resolve(__dirname, 'app/src/web/electron.ts') },
      // The SDK speaks JSON-RPC over the Node stream classes. Vite would pick
      // the package's browser build (no stream classes) through its `browser`
      // field, so the Node entry is named directly.
      {
        find: /^vscode-jsonrpc\/node(\.js)?$/,
        replacement: resolveCopilotJsonRpc(path.resolve(__dirname, 'app')),
      },
      { find: /^koffi$/, replacement: path.resolve(__dirname, 'app/src/web/koffi.ts') },
      { find: /^desktop-url-impl$/, replacement: legacyUrlPackage },
      { find: /^desktop-crypto-impl$/, replacement: legacyCryptoPackage },
      { find: /^desktop-stream-impl$/, replacement: legacyStreamPackage },
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
        loadPaths: sassLoadPaths(__dirname),
      },
    },
  },
  build: {
    outDir,
    emptyOutDir: process.env.DESKTOP_VITE_DIST === undefined,
    // Off for the distributed bundle; see moveSourceMaps().
    sourcemap: process.env.DESKTOP_VITE_SOURCEMAP ? true : false,
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
