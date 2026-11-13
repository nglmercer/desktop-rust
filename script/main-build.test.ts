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
import { build } from 'vite'
import { describe, it } from 'vitest'
import {
  bundleDevtools,
  mainNativeModules,
  moduleId,
  nativeRequires,
} from '../vite.main.config'

describe('Vite Electron main build', () => {
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
