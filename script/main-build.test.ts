// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { builtinModules, createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import ts from 'typescript'
import { build } from 'vite'
import { describe, it } from 'vitest'
import webpack from 'webpack'
import developmentConfigs from '../app/webpack.development'
import productionConfigs from '../app/webpack.production'
import packageInfo from '../package.json'
import mainConfig, {
  bundleDevtools,
  mainNativeModules,
  moduleId,
  nativeRequires,
} from '../vite.main.config'
import { createRendererDevServer } from './renderer-dev-server'

describe('Vite Electron main build', () => {
  it('uses Vite for main and keeps only the renderer in Webpack', () => {
    for (const command of [
      packageInfo.scripts['compile:dev'],
      packageInfo.scripts['compile:prod'],
    ]) {
      assert.match(command, /vite build -c vite\.main\.config\.ts/)
      assert.match(
        command,
        /webpack --config app\/webpack\.(?:development|production)\.ts/
      )
      assert.ok(
        command.indexOf('vite.main.config.ts') <
          command.indexOf('webpack --config')
      )
    }
    for (const configs of [developmentConfigs, productionConfigs]) {
      assert.equal(configs.length, 1)
      const config = configs[0]
      assert.equal(config.target, 'electron-renderer')
      assert.ok(
        config.entry &&
          typeof config.entry === 'object' &&
          'renderer' in config.entry
      )
      assert.ok(!('main' in config.entry))
    }
  })

  it('type-checks the full project as well as the bundled renderer', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-renderer-types-'))
    try {
      const entry = path.join(root, 'renderer.ts')
      const configFile = path.join(root, 'tsconfig.json')
      writeFileSync(
        configFile,
        JSON.stringify({
          compilerOptions: {
            strict: true,
            types: [],
            target: 'ES2022',
            module: 'ESNext',
          },
          include: ['*.ts'],
        })
      )
      writeFileSync(
        path.join(root, 'unrelated-test.ts'),
        'const other: string = 123'
      )
      const rule = developmentConfigs[0].module?.rules?.[0]
      assert.ok(
        rule &&
          typeof rule === 'object' &&
          'use' in rule &&
          Array.isArray(rule.use)
      )
      const loader = rule.use.find(
        item =>
          typeof item === 'object' &&
          item !== null &&
          'loader' in item &&
          item.loader === 'ts-loader'
      )
      assert.ok(loader && typeof loader === 'object')
      const loaderOptions =
        'options' in loader &&
        loader.options !== null &&
        typeof loader.options === 'object'
          ? loader.options
          : {}
      assert.notEqual(loaderOptions.onlyCompileBundledFiles, true)
      const compile = async () => {
        const compiler = webpack({
          mode: 'development',
          entry,
          output: { path: path.join(root, 'out'), filename: 'renderer.js' },
          module: {
            rules: [
              {
                test: /\.ts$/,
                use: [
                  {
                    loader: require.resolve('ts-loader'),
                    options: { ...loaderOptions, configFile },
                  },
                ],
              },
            ],
          },
        })
        try {
          return await new Promise<webpack.Stats>((resolve, reject) => {
            compiler.run((error, stats) => {
              if (error) reject(error)
              else if (stats === undefined)
                reject(new Error('Webpack returned no statistics'))
              else resolve(stats)
            })
          })
        } finally {
          await new Promise<void>((resolve, reject) =>
            compiler.close(error => (error ? reject(error) : resolve()))
          )
        }
      }
      writeFileSync(entry, 'const value: string = "valid"; console.log(value)')
      const invalidUnbundled = await compile()
      assert.equal(invalidUnbundled.hasErrors(), true)
      assert.match(
        invalidUnbundled.toString('errors-only'),
        /unrelated-test\.ts/
      )
      assert.match(invalidUnbundled.toString('errors-only'), /TS2322/)
      writeFileSync(
        path.join(root, 'unrelated-test.ts'),
        'const other: string = "valid"'
      )
      const valid = await compile()
      assert.equal(valid.hasErrors(), false, valid.toString('errors-only'))
      writeFileSync(entry, 'const value: string = 123; console.log(value)')
      const invalid = await compile()
      assert.equal(invalid.hasErrors(), true)
      assert.match(invalid.toString('errors-only'), /TS2322/)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  }, 60_000)

  it('serves the renderer bundle and hot-reload stream in development', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-renderer-dev-'))
    let devServer: ReturnType<typeof createRendererDevServer> | undefined
    try {
      const entry = path.join(root, 'entry.js')
      const confetti = require.resolve('react-confetti', {
        paths: [path.resolve('app/node_modules')],
      })
      const esmConfetti = path.join(
        path.dirname(confetti),
        'react-confetti.mjs'
      )
      writeFileSync(
        entry,
        `import Confetti from ${JSON.stringify(
          esmConfetti
        )}; console.log("renderer-development-fixture", Confetti)`
      )
      const original = developmentConfigs[0]
      assert.ok(
        original.entry &&
          typeof original.entry === 'object' &&
          'renderer' in original.entry
      )
      const rendererEntry = original.entry.renderer
      assert.ok(Array.isArray(rendererEntry))
      assert.match(
        rendererEntry[0],
        /webpack-hot-middleware\/client\?path=http:\/\/localhost:\d+\/__webpack_hmr/
      )
      const runningServer = createRendererDevServer({
        ...original,
        entry: { renderer: [rendererEntry[0], entry] },
        output: { ...original.output, path: path.join(root, 'output') },
      })
      devServer = runningServer
      await new Promise<void>((resolve, reject) => {
        runningServer.server.once('error', reject)
        runningServer.server.listen(0, '127.0.0.1', resolve)
      })
      const address = runningServer.server.address()
      assert.ok(address && typeof address === 'object')
      const origin = `http://127.0.0.1:${address.port}`
      const response = await fetch(`${origin}/build/renderer.js`, {
        signal: AbortSignal.timeout(30_000),
      })
      assert.equal(response.status, 200)
      assert.match(await response.text(), /renderer-development-fixture/)
      const hmr = await fetch(`${origin}/__webpack_hmr`, {
        signal: AbortSignal.timeout(10_000),
      })
      assert.equal(hmr.status, 200)
      assert.match(hmr.headers.get('content-type') ?? '', /^text\/event-stream/)
      const stream = hmr.body?.getReader()
      assert.ok(stream)
      const first = await stream.read()
      assert.match(new TextDecoder().decode(first.value), /(?:sync|built)/)
      await stream.cancel()
    } finally {
      if (devServer !== undefined) await devServer.close()
      rmSync(root, { recursive: true, force: true })
    }
  }, 60_000)

  for (const platform of ['darwin', 'win32'] as const) {
    it(`bundles the whole ${platform} main graph without machine-local imports`, async () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-main-platform-'))
      try {
        const fakeBinary = path.join(root, 'native.node')
        const output = path.join(root, 'output')
        writeFileSync(fakeBinary, 'native fixture')
        await build({
          ...mainConfig,
          configFile: false,
          logLevel: 'silent',
          plugins: [
            mainNativeModules(platform, {
              'desktop-notifications': fakeBinary,
              registry: fakeBinary,
            }),
            bundleDevtools(),
          ],
          define: {
            ...mainConfig.define,
            __DARWIN__: platform === 'darwin',
            __WIN32__: platform === 'win32',
            __LINUX__: false,
            __DEV__: true,
            'process.platform': JSON.stringify(platform),
          },
          build: { ...mainConfig.build, outDir: output, minify: false },
        })
        const bundle = readFileSync(path.join(output, 'main.js'), 'utf8')
        const source = ts.createSourceFile(
          'main.js',
          bundle,
          ts.ScriptTarget.ESNext,
          true,
          ts.ScriptKind.JS
        )
        const allowed = new Set([
          ...builtinModules,
          'electron',
          '7zip',
          './desktop-notifications.node',
          ...(platform === 'win32' ? ['./registry.node'] : []),
        ])
        const required = new Set<string>()
        const visit = (node: ts.Node): void => {
          if (
            ts.isCallExpression(node) &&
            ts.isIdentifier(node.expression) &&
            node.expression.text === 'require'
          ) {
            const argument = node.arguments[0]
            assert.ok(
              argument && ts.isStringLiteral(argument),
              'Dynamic require in bundled main'
            )
            assert.ok(
              allowed.has(argument.text) || argument.text.startsWith('node:'),
              `Unexpected main-process dependency: ${argument.text}`
            )
            required.add(argument.text)
          }
          ts.forEachChild(node, visit)
        }
        visit(source)
        assert.ok(required.has('./desktop-notifications.node'))
        assert.equal(required.has('./registry.node'), platform === 'win32')
        assert.equal(
          readFileSync(path.join(output, 'desktop-notifications.node'), 'utf8'),
          'native fixture'
        )
        if (platform === 'win32')
          assert.equal(
            readFileSync(path.join(output, 'registry.node'), 'utf8'),
            'native fixture'
          )
        assert.ok(!bundle.includes(fakeBinary))
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }, 90_000)
  }

  it('normalizes Windows module paths the same way as Vite IDs', () => {
    assert.equal(
      moduleId('C:\\workspace\\app\\src\\main.ts', 'win32'),
      moduleId('C:/workspace/app/src/main.ts', 'win32')
    )
    assert.notEqual(
      moduleId('/repo/file\\name.ts', 'linux'),
      moduleId('/repo/file/name.ts', 'linux')
    )
  })

  for (const { channel, nodeEnv, isDevelopment } of [
    { channel: 'development', nodeEnv: 'development', isDevelopment: true },
    { channel: 'production', nodeEnv: 'production', isDevelopment: false },
    { channel: 'development', nodeEnv: 'production', isDevelopment: true },
    { channel: 'beta', nodeEnv: 'development', isDevelopment: false },
  ] as const) {
    it(`builds ${channel} for NODE_ENV=${nodeEnv} without deleting adjacent files`, () => {
      const output = mkdtempSync(path.join(tmpdir(), 'desktop-main-build-'))
      try {
        writeFileSync(path.join(output, 'sibling.js'), 'preserve')
        const result = spawnSync(
          process.execPath,
          [
            'node_modules/vite/bin/vite.js',
            'build',
            '-c',
            'vite.main.config.ts',
            '--outDir',
            output,
          ],
          {
            encoding: 'utf8',
            timeout: 60_000,
            env: {
              ...process.env,
              NODE_ENV: nodeEnv,
              RELEASE_CHANNEL: channel,
            },
          }
        )
        assert.equal(result.status, 0, result.stderr)
        assert.equal(
          readFileSync(path.join(output, 'sibling.js'), 'utf8'),
          'preserve'
        )
        const map = JSON.parse(
          readFileSync(path.join(output, 'main.js.map'), 'utf8')
        ) as { sources: string[] }
        const sources = map.sources.map(source => source.replaceAll('\\', '/'))
        assert.ok(sources.some(source => source.endsWith('/logform/index.js')))
        assert.ok(
          !sources.some(source => source.endsWith('/logform/dist/browser.js'))
        )
        assert.equal(
          sources.some(source =>
            source.includes('/electron-devtools-installer/dist/index.js')
          ),
          isDevelopment
        )
        const bundle = readFileSync(path.join(output, 'main.js'), 'utf8')
        assert.ok(
          !/require\(["']electron-devtools-installer["']\)/.test(bundle)
        )
        if (isDevelopment)
          assert.ok(
            bundle.includes('electron-devtools-installer can only be used')
          )
        else
          assert.ok(
            !bundle.includes('electron-devtools-installer can only be used')
          )
      } finally {
        rmSync(output, { recursive: true, force: true })
      }
    }, 90_000)
  }

  it('loads development tools only when requested', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-main-devtools-'))
    try {
      const appRequire = createRequire(path.resolve('app/package.json'))
      const installer = appRequire.resolve('electron-devtools-installer')
      const entry = path.join(root, 'entry.js')
      const output = path.join(root, 'out')
      writeFileSync(
        entry,
        `import load from 'virtual:desktop-devtools-loader'; export { load };`
      )
      await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [
          bundleDevtools(),
          {
            name: 'observe-installer-load',
            enforce: 'pre',
            transform(code, id) {
              if (moduleId(id) === moduleId(installer))
                return `globalThis.__installerLoads++;\n${code}`
              return null
            },
          },
        ],
        resolve: { mainFields: ['module', 'main'], conditions: ['node'] },
        build: {
          outDir: output,
          minify: false,
          commonjsOptions: { strictRequires: true },
          lib: { entry, formats: ['cjs'], fileName: () => 'fixture.cjs' },
          rollupOptions: {
            external: id =>
              id === 'electron' ||
              id.startsWith('node:') ||
              builtinModules.includes(id),
          },
        },
      })
      writeFileSync(
        path.join(output, 'runner.cjs'),
        `
const assert = require('node:assert/strict');
const Module = require('node:module');
const originalLoad = Module._load;
Module._load = function(id, ...args) { return id === 'electron' ? {} : originalLoad.call(this, id, ...args); };
globalThis.__installerLoads = 0;
const bundle = require('./fixture.cjs');
assert.equal(globalThis.__installerLoads, 0);
assert.equal(typeof bundle.load().default, 'function');
assert.equal(globalThis.__installerLoads, 1);
bundle.load();
assert.equal(globalThis.__installerLoads, 1);
`
      )
      const runtime = spawnSync(
        process.execPath,
        [path.join(output, 'runner.cjs')],
        { encoding: 'utf8', timeout: 10_000 }
      )
      assert.equal(runtime.status, 0, runtime.stderr)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  }, 60_000)

  it('preserves conditional native loads after relocation', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-main-native-'))
    try {
      const appRequire = createRequire(path.resolve('app/package.json'))
      const notifications = appRequire.resolve(
        'desktop-notifications/dist/native-module.js'
      )
      const registry = appRequire.resolve('registry-js/dist/lib/registry.js')
      const entry = path.join(root, 'entry.js')
      const binaries = path.join(root, 'binaries')
      const output = path.join(root, 'build')
      const relocated = path.join(root, 'relocated')
      mkdirSync(binaries)
      const notificationsBinary = path.join(binaries, 'notifications.node')
      const registryBinary = path.join(binaries, 'registry.node')
      writeFileSync(notificationsBinary, 'notifications fixture')
      writeFileSync(registryBinary, 'registry fixture')
      writeFileSync(
        entry,
        `import * as notifications from ${JSON.stringify(
          notifications
        )};\nimport * as registry from ${JSON.stringify(
          registry
        )};\nexport { notifications, registry };\n`
      )
      await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [
          mainNativeModules('win32', {
            'desktop-notifications': notificationsBinary,
            registry: registryBinary,
          }),
        ],
        resolve: { mainFields: ['module', 'main'], conditions: ['node'] },
        build: {
          outDir: output,
          copyPublicDir: false,
          minify: false,
          commonjsOptions: { ignore: nativeRequires },
          lib: { entry, formats: ['cjs'], fileName: () => 'fixture.cjs' },
          rollupOptions: {
            external: id =>
              id.startsWith('node:') || builtinModules.includes(id),
          },
        },
      })
      renameSync(output, relocated)
      assert.equal(
        readFileSync(
          path.join(relocated, 'desktop-notifications.node'),
          'utf8'
        ),
        'notifications fixture'
      )
      assert.equal(
        readFileSync(path.join(relocated, 'registry.node'), 'utf8'),
        'registry fixture'
      )
      const bundle = readFileSync(path.join(relocated, 'fixture.cjs'), 'utf8')
      assert.ok(!bundle.includes(notificationsBinary))
      assert.ok(!bundle.includes(registryBinary))
      writeFileSync(
        path.join(relocated, 'runner.cjs'),
        `
const assert = require('node:assert/strict');
const path = require('node:path');
const Module = require('node:module');
const os = require('node:os');
Object.defineProperty(process, 'platform', { value: 'win32' });
os.release = () => '10.0.19045';
const loaded = [];
Module._extensions['.node'] = (module, filename) => {
  assert.equal(path.dirname(filename), __dirname);
  loaded.push(path.basename(filename));
  module.exports = { initializeNotifications() {} };
};
const bundle = require('./fixture.cjs');
assert.deepEqual(loaded, ['registry.node']);
bundle.notifications.initializeNotifications({});
bundle.notifications.initializeNotifications({});
assert.deepEqual(loaded, ['registry.node', 'desktop-notifications.node']);
`
      )
      const runtime = spawnSync(
        process.execPath,
        [path.join(relocated, 'runner.cjs')],
        { encoding: 'utf8', timeout: 10_000 }
      )
      assert.equal(runtime.status, 0, runtime.stderr)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  }, 60_000)

  it('rejects builds missing required platform addons', async () => {
    const root = mkdtempSync(
      path.join(tmpdir(), 'desktop-main-missing-native-')
    )
    try {
      const entry = path.join(root, 'entry.js')
      writeFileSync(entry, 'export default 1')
      await assert.rejects(
        build({
          configFile: false,
          logLevel: 'silent',
          plugins: [
            mainNativeModules('win32', {
              'desktop-notifications': path.join(root, 'missing.node'),
            }),
          ],
          build: {
            outDir: path.join(root, 'out'),
            lib: { entry, formats: ['cjs'] },
          },
        }),
        /Missing required native module desktop-notifications/
      )
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
})
