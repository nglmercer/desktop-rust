// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import * as path from 'node:path'
import { afterEach, describe, it } from 'vitest'
import { buildRustProduction } from './build-rust-production'
import {
  createIsolatedViteOutput,
  requireAbsentViteOutputFiles,
  requireEmptyViteOutput,
  viteOutputDirectory,
} from './vite-output'

const directories: string[] = []
afterEach(() => {
  for (const directory of directories.splice(0)) {
    rmSync(directory, { recursive: true, force: true })
  }
})

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'production-build-test-'))
  directories.push(root)
  const vite = path.join(root, 'node_modules/vite/bin/vite.js')
  const bin = path.join(root, 'bin')
  const output = path.join(root, 'out/vite-dist/index.html')
  const capture = path.join(root, 'capture')
  mkdirSync(path.dirname(vite), { recursive: true })
  mkdirSync(bin)
  mkdirSync(path.dirname(output), { recursive: true })
  writeFileSync(output, 'live development bundle')
  writeFileSync(
    vite,
    `
    const fs = require('node:fs');
    const path = require('node:path');
    const directory = process.env.DESKTOP_VITE_DIST;
    const config = process.argv.includes('vite.highlighter.config.ts') ? 'highlighter.js'
      : process.argv.includes('vite.ipc-shim.config.ts') ? 'ipc-shim.js' : 'index.html';
    fs.appendFileSync(process.env.CAPTURE, JSON.stringify({kind:'vite', directory, config, pid:process.pid, nodeEnv:process.env.NODE_ENV, channel:process.env.RELEASE_CHANNEL}) + '\\n');
    if (process.env.WAIT_VITE) { setInterval(() => {}, 1000); return; }
    fs.writeFileSync(path.join(directory, config), config);
    if (process.env.FAIL_VITE === config) process.exit(2);
  `
  )
  const cargo = path.join(bin, 'cargo')
  writeFileSync(
    cargo,
    `#!/usr/bin/env node
    const fs = require('node:fs');
    const path = require('node:path');
    const directory = process.env.DESKTOP_VITE_DIST;
    for (const file of ['index.html', 'highlighter.js', 'ipc-shim.js']) {
      if (!fs.existsSync(path.join(directory, file))) process.exit(3);
    }
    fs.appendFileSync(process.env.CAPTURE, JSON.stringify({kind:'cargo', directory, args:process.argv.slice(2), native:process.env.DESKTOP_REQUIRE_NATIVE_PLATFORM}) + '\\n');
    if (process.env.FAIL_CARGO) process.exit(4);
  `
  )
  chmodSync(cargo, 0o755)
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    PATH: `${bin}${path.delimiter}${process.env.PATH}`,
    CAPTURE: capture,
  }
  delete env.DESKTOP_VITE_DIST
  return { root, env, output, capture }
}

function records(file: string): Array<{
  kind: string
  directory: string
  config?: string
  args?: string[]
  nodeEnv?: string
  channel?: string
  native?: string
  pid?: number
}> {
  return readFileSync(file, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map(line => JSON.parse(line))
}

async function bounded<T>(
  promise: Promise<T>,
  milliseconds: number
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Production build did not stop')),
          milliseconds
        )
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

describe.skipIf(process.platform === 'win32')(
  'isolated Rust production build',
  () => {
    it('does not delete a replacement directory or a symlink during cleanup', () => {
      const { root } = fixture()
      const output = createIsolatedViteOutput()
      const temporary = path.dirname(output.directory)
      const saved = `${temporary}-saved`
      const foreign = path.join(root, 'foreign')
      mkdirSync(foreign)
      writeFileSync(path.join(foreign, 'keep'), 'important')
      try {
        renameSync(temporary, saved)
        symlinkSync(foreign, temporary, 'dir')
        assert.throws(() => output.dispose(), /directory changed/)
        assert.equal(
          readFileSync(path.join(foreign, 'keep'), 'utf8'),
          'important'
        )
        rmSync(temporary)
        mkdirSync(temporary)
        writeFileSync(path.join(temporary, 'keep'), 'replacement')
        assert.throws(() => output.dispose(), /directory changed/)
        assert.equal(
          readFileSync(path.join(temporary, 'keep'), 'utf8'),
          'replacement'
        )
      } finally {
        rmSync(temporary, { recursive: true, force: true })
        rmSync(saved, { recursive: true, force: true })
      }
    })

    it('refuses unsafe paths, development aliases and nonempty output directories', () => {
      const { root } = fixture()
      const development = path.join(root, 'out/vite-dist')
      const privateOutput = path.join(root, 'private-output')
      mkdirSync(privateOutput)
      assert.equal(viteOutputDirectory(root, {}), development)
      assert.equal(
        viteOutputDirectory(root, { DESKTOP_VITE_DIST: privateOutput }),
        privateOutput
      )
      assert.throws(
        () => viteOutputDirectory(root, { DESKTOP_VITE_DIST: 'relative' }),
        /absolute/
      )
      assert.throws(
        () => viteOutputDirectory(root, { DESKTOP_VITE_DIST: development }),
        /overlap/
      )
      assert.throws(
        () =>
          viteOutputDirectory(root, {
            DESKTOP_VITE_DIST: path.dirname(development),
          }),
        /overlap/
      )
      const alias = path.join(root, 'alias')
      symlinkSync(development, alias)
      assert.throws(
        () => viteOutputDirectory(root, { DESKTOP_VITE_DIST: alias }),
        /overlap/
      )
      assert.throws(
        () =>
          viteOutputDirectory(root, {
            DESKTOP_VITE_DIST: path.join(alias, 'child'),
          }),
        /overlap/
      )
      requireAbsentViteOutputFiles(privateOutput, [
        'highlighter.js',
        'highlighter.js.map',
      ])
      writeFileSync(path.join(privateOutput, 'highlighter.js.map'), 'keep')
      assert.throws(
        () =>
          requireAbsentViteOutputFiles(privateOutput, [
            'highlighter.js',
            'highlighter.js.map',
          ]),
        /already contains/
      )
      rmSync(path.join(privateOutput, 'highlighter.js.map'))
      if (process.platform !== 'win32') {
        symlinkSync(
          path.join(root, 'missing'),
          path.join(privateOutput, 'highlighter.js')
        )
        assert.throws(
          () => requireAbsentViteOutputFiles(privateOutput, ['highlighter.js']),
          /already contains/
        )
        rmSync(path.join(privateOutput, 'highlighter.js'))
      }
      requireEmptyViteOutput(privateOutput)
      writeFileSync(path.join(privateOutput, 'keep'), 'foreign data')
      assert.throws(() => requireEmptyViteOutput(privateOutput), /new, empty/)
      assert.equal(
        readFileSync(path.join(privateOutput, 'keep'), 'utf8'),
        'foreign data'
      )
      const link = path.join(root, 'private-link')
      symlinkSync(privateOutput, link)
      assert.throws(() => requireEmptyViteOutput(link), /new, empty/)
    })

    it('builds all assets and Rust binaries from a fresh bundle without touching development output', async () => {
      const { root, env, output, capture } = fixture()
      await buildRustProduction(root, env)
      const entries = records(capture)
      assert.deepEqual(
        entries.map(entry => entry.kind),
        ['vite', 'vite', 'vite', 'cargo']
      )
      assert.deepEqual(
        entries.slice(0, 3).map(entry => entry.config),
        ['index.html', 'highlighter.js', 'ipc-shim.js']
      )
      const bundle = entries[0].directory
      assert.ok(path.isAbsolute(bundle))
      assert.ok(entries.every(entry => entry.directory === bundle))
      assert.ok(
        entries
          .slice(0, 3)
          .every(
            entry =>
              entry.nodeEnv === 'production' && entry.channel === 'production'
          )
      )
      assert.equal(entries[3].native, '1')
      assert.ok(
        entries[3].args?.includes(
          'desktop-app/packaged-ui,desktop-cli/embedded-ui,desktop-cli/packaged-runtime'
        )
      )
      assert.equal(readFileSync(output, 'utf8'), 'live development bundle')
      assert.equal(existsSync(path.dirname(bundle)), false)
    })

    it('removes only its temporary bundle when a build fails', async () => {
      const { root, env, output, capture } = fixture()
      await assert.rejects(
        buildRustProduction(root, { ...env, FAIL_CARGO: '1' }),
        /cargo failed/
      )
      const entries = records(capture)
      assert.equal(existsSync(path.dirname(entries[0].directory)), false)
      assert.equal(readFileSync(output, 'utf8'), 'live development bundle')
    })

    it('uses different output directories for concurrent builds', async () => {
      const { root, env, output, capture } = fixture()
      await Promise.all([
        buildRustProduction(root, env),
        buildRustProduction(root, env),
      ])
      const entries = records(capture)
      const bundles = new Set(entries.map(entry => entry.directory))
      assert.equal(bundles.size, 2)
      for (const bundle of bundles)
        assert.equal(existsSync(path.dirname(bundle)), false)
      assert.equal(readFileSync(output, 'utf8'), 'live development bundle')
    })

    it('rejects an external bundle override before executing a build', async () => {
      const { root, env, capture } = fixture()
      await assert.rejects(
        buildRustProduction(root, { ...env, DESKTOP_VITE_DIST: root }),
        /manages its own/
      )
      assert.equal(existsSync(capture), false)
    })

    it('stops a running build and removes its private bundle on interrupt', async () => {
      const { root, env, capture, output } = fixture()
      const driver = path.join(root, 'driver.ts')
      writeFileSync(
        driver,
        `import { buildRustProduction } from ${JSON.stringify(
          path.resolve('script/build-rust-production.ts')
        )}\nvoid buildRustProduction(${JSON.stringify(
          root
        )}).catch(() => { process.exitCode = 1 })\n`
      )
      const child = spawn(process.execPath, ['--import', 'tsx', driver], {
        cwd: process.cwd(),
        env: { ...env, WAIT_VITE: '1' },
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      const closed = new Promise<{
        code: number | null
        signal: NodeJS.Signals | null
      }>((resolve, reject) => {
        child.once('error', reject)
        child.once('close', (code, signal) => resolve({ code, signal }))
      })
      let buildPid: number | undefined
      try {
        const deadline = Date.now() + 10_000
        while (!existsSync(capture) && Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 25))
        }
        assert.equal(existsSync(capture), true)
        const entry = records(capture)[0]
        const actualPid = entry.pid
        assert.ok(actualPid)
        buildPid = actualPid
        assert.equal(child.kill('SIGINT'), true)
        const result = await bounded(closed, 10_000)
        assert.notEqual(result.code, 0)
        assert.equal(existsSync(path.dirname(entry.directory)), false)
        assert.equal(readFileSync(output, 'utf8'), 'live development bundle')
        assert.throws(() => process.kill(actualPid, 0), { code: 'ESRCH' })
      } finally {
        if (child.exitCode === null && child.signalCode === null)
          child.kill('SIGKILL')
        if (buildPid && child.exitCode === null && child.signalCode === null) {
          try {
            process.kill(-buildPid, 'SIGKILL')
          } catch {}
        }
        await closed.catch(() => {})
      }
    }, 20_000)
  }
)
