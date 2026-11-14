// @vitest-environment node
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { build } from 'vite'
import { describe, it } from 'vitest'
import {
  rendererModuleId,
  rendererNativeAddons,
  type IRendererNativeAddon,
} from './renderer-vite-native-addons'

describe('Vite renderer native addons', () => {
  it('normalizes Windows module IDs without changing Unix backslashes', () => {
    assert.equal(
      rendererModuleId('C:\\project\\keytar\\loader.js', 'win32'),
      rendererModuleId('C:/project/keytar/loader.js', 'win32')
    )
    assert.notEqual(
      rendererModuleId('/project/file\\name.js', 'linux'),
      rendererModuleId('/project/file/name.js', 'linux')
    )
  })

  it('loads both addons from the relocated bundle', async () => {
    const root = await mkdtemp(path.join(tmpdir(), 'desktop-vite-addons-'))
    try {
      const addons: IRendererNativeAddon[] = ['one', 'two'].map(name => ({
        name,
        loader: path.join(root, `${name}.cjs`),
        pattern: `./native/${name}.node`,
      }))
      const overrides: Record<string, string> = {}
      for (const addon of addons) {
        await writeFile(
          addon.loader,
          `module.exports = require('${addon.pattern}')`
        )
        overrides[addon.name] = path.join(root, `${addon.name}.node`)
        await writeFile(overrides[addon.name], `binary ${addon.name}`)
      }
      const entry = path.join(root, 'entry.js')
      await writeFile(
        entry,
        `import one from './one.cjs'; import two from './two.cjs'; export { one, two }`
      )
      const output = path.join(root, 'out')
      await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [rendererNativeAddons(addons, overrides)],
        build: {
          outDir: output,
          lib: { entry, formats: ['cjs'], fileName: () => 'bundle.cjs' },
          rollupOptions: {
            external: id => id.startsWith('desktop-native-addon:'),
            output: {
              paths: id => `./${id.slice('desktop-native-addon:'.length)}.node`,
            },
          },
        },
      })
      const relocated = path.join(root, 'relocated')
      await rename(output, relocated)
      assert.equal(
        await readFile(path.join(relocated, 'one.node'), 'utf8'),
        'binary one'
      )
      assert.equal(
        await readFile(path.join(relocated, 'two.node'), 'utf8'),
        'binary two'
      )
      const runner = path.join(relocated, 'runner.cjs')
      await writeFile(
        runner,
        `
const assert = require('node:assert/strict');
const path = require('node:path');
require('node:module')._extensions['.node'] = (module, file) => {
  assert.equal(path.dirname(file), __dirname);
  module.exports = path.basename(file);
};
const bundle = require('./bundle.cjs');
assert.equal(bundle.one, 'one.node');
assert.equal(bundle.two, 'two.node');
`
      )
      const run = spawnSync(process.execPath, [runner], { encoding: 'utf8' })
      assert.equal(run.status, 0, run.stderr)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rebuilds when the native binary changes without requiring another transform', async () => {
    const root = await mkdtemp(path.join(tmpdir(), 'desktop-vite-watch-'))
    try {
      const loader = path.join(root, 'loader.cjs')
      const entry = path.join(root, 'entry.js')
      const binary = path.join(root, 'addon.node')
      const output = path.join(root, 'out')
      const addon = { name: 'addon', loader, pattern: './native/addon.node' }
      await writeFile(loader, "module.exports = require('./native/addon.node')")
      await writeFile(
        entry,
        "import addon from './loader.cjs'; export { addon }"
      )
      await writeFile(binary, 'first')
      const watcher = await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [rendererNativeAddons([addon], { addon: binary })],
        build: {
          outDir: output,
          watch: {},
          lib: { entry, formats: ['cjs'] },
          rollupOptions: {
            external: id => id.startsWith('desktop-native-addon:'),
          },
        },
      })
      if (!('on' in watcher)) throw new Error('Expected Vite watcher')
      const waitForBuild = () =>
        new Promise<void>((resolve, reject) => {
          const timer = setTimeout(
            () => reject(new Error('Build timed out')),
            10_000
          )
          watcher.on('event', function listener(event) {
            if (event.code !== 'END' && event.code !== 'ERROR') return
            clearTimeout(timer)
            watcher.off('event', listener)
            if (event.code === 'ERROR') reject(event.error)
            else resolve()
          })
        })
      try {
        await waitForBuild()
        const rebuilt = waitForBuild()
        await writeFile(binary, 'second')
        await rebuilt
        assert.equal(
          await readFile(path.join(output, 'addon.node'), 'utf8'),
          'second'
        )
        const removedLoader = waitForBuild()
        await writeFile(entry, 'export const value = 1')
        await assert.rejects(
          removedLoader,
          /Native loader addon was not processed/
        )
      } finally {
        await watcher.close()
      }
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  }, 20_000)

  it('fails closed for a changed, duplicated, or unprocessed loader', async () => {
    const root = await mkdtemp(
      path.join(tmpdir(), 'desktop-vite-addon-errors-')
    )
    try {
      const loader = path.join(root, 'loader.cjs')
      const entry = path.join(root, 'entry.js')
      const binary = path.join(root, 'addon.node')
      const addon = { name: 'addon', loader, pattern: './native/addon.node' }
      await writeFile(binary, 'fixture')
      const compile = () =>
        build({
          configFile: false,
          logLevel: 'silent',
          plugins: [rendererNativeAddons([addon], { addon: binary })],
          build: {
            outDir: path.join(root, 'out'),
            lib: { entry, formats: ['cjs'] },
            rollupOptions: {
              external: id => id.startsWith('desktop-native-addon:'),
            },
          },
        })
      await writeFile(entry, "import './loader.cjs'")
      await writeFile(loader, "module.exports = require('./different.node')")
      await assert.rejects(compile(), /Unexpected addon native loader/)
      await writeFile(
        loader,
        "require('./native/addon.node'); require('./native/addon.node')"
      )
      await assert.rejects(compile(), /Unexpected addon native loader/)
      await writeFile(entry, 'export const value = 1')
      await assert.rejects(compile(), /Native loader addon was not processed/)
      await writeFile(entry, `import ${JSON.stringify(loader)}`)
      await assert.rejects(
        build({
          configFile: false,
          logLevel: 'silent',
          plugins: [rendererNativeAddons([addon], { addon: binary })],
          build: {
            outDir: path.join(root, 'out'),
            lib: { entry, formats: ['cjs'] },
            rollupOptions: { external: [loader] },
          },
        }),
        /Native loader addon was not processed/
      )
      await assert.rejects(
        build({
          configFile: false,
          logLevel: 'silent',
          plugins: [
            rendererNativeAddons([addon], {
              addon: path.join(root, 'missing.node'),
            }),
          ],
          build: { lib: { entry, formats: ['cjs'] } },
        }),
        /Missing addon addon/
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
