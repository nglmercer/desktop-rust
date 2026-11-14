// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'vitest'
import { watchRustBundle, type DevWatcher } from './rust-dev-watcher'
import { stopDevelopmentBuild } from './rust-dev-process'
import { copyStaticResources } from '../vite.config'

const enabled =
  process.env.DESKTOP_TEST_RUST_DEV === '1' &&
  process.platform === 'linux' &&
  !!process.env.DISPLAY

function startHost(args: ReadonlyArray<string> = []) {
  const directory = mkdtempSync(path.join(tmpdir(), 'desktop-rust-dev-test-'))
  const runtime = path.join(directory, 'runtime')
  mkdirSync(runtime, { mode: 0o700 })
  const env = Object.fromEntries(
    Object.entries(process.env).filter(
      ([name]) => !name.startsWith('GIT_') && !name.startsWith('DESKTOP_')
    )
  )
  const child = spawn(
    process.execPath,
    ['--import', 'tsx', 'script/start-rust.ts', ...args],
    {
      cwd: process.cwd(),
      env: {
        ...env,
        NODE_ENV: 'development',
        DESKTOP_USER_DATA: path.join(directory, 'user'),
        XDG_CONFIG_HOME: path.join(directory, 'config'),
        XDG_DATA_HOME: path.join(directory, 'data'),
        XDG_CACHE_HOME: path.join(directory, 'cache'),
        XDG_RUNTIME_DIR: runtime,
        DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(
          directory,
          'no-dbus'
        )}`,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(directory, 'gitconfig'),
        SHELL: '/bin/bash',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  )
  let output = ''
  child.stdout.on('data', chunk => {
    output = (output + String(chunk)).slice(-100_000)
  })
  child.stderr.on('data', chunk => {
    output = (output + String(chunk)).slice(-100_000)
  })
  const closed = new Promise<{
    code: number | null
    signal: NodeJS.Signals | null
  }>((resolve, reject) => {
    child.once('error', reject)
    child.once('close', (code, signal) => resolve({ code, signal }))
  })
  return { child, directory, closed, output: () => output }
}

async function bounded<T>(promise: Promise<T>, timeout: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error('Development launcher timed out')),
          timeout
        )
      }),
    ])
  } finally {
    if (timer) {
      clearTimeout(timer)
    }
  }
}

function nextBuild(watcher: DevWatcher): Promise<void> {
  return new Promise((resolve, reject) => {
    watcher.on('event', function listener(event) {
      if (event.code !== 'END' && event.code !== 'ERROR') {
        return
      }
      watcher.off('event', listener)
      if (event.code === 'ERROR') {
        reject(event.error)
      } else {
        resolve()
      }
    })
  })
}

function staticFixture(directory: string) {
  const project = path.join(directory, 'project')
  const renderer = path.join(directory, 'renderer')
  const output = path.join(directory, 'out')
  for (const name of [
    'gemoji/db',
    'gemoji/images/emoji',
    'app/static/common/gitignore',
    'app/static/common/choosealicense.com/_licenses',
    'app/static/linux',
  ]) {
    mkdirSync(path.join(project, name), { recursive: true })
  }
  mkdirSync(renderer)
  mkdirSync(output)
  writeFileSync(
    path.join(renderer, 'entry.ts'),
    "export const marker = 'fixture'\n"
  )
  writeFileSync(path.join(project, 'gemoji/db/emoji.json'), '{}')
  writeFileSync(
    path.join(project, 'gemoji/images/emoji/test.png'),
    'emoji-before'
  )
  writeFileSync(
    path.join(project, 'app/static/common/gitignore/Example.gitignore'),
    'ignored'
  )
  if (process.platform !== 'win32') {
    const templates = path.join(project, 'app/static/common/gitignore')
    writeFileSync(path.join(templates, 'linked-template'), 'linked-before')
    symlinkSync('linked-template', path.join(templates, 'Linked.gitignore'))
  }
  writeFileSync(
    path.join(project, 'app/static/common/choosealicense.com/LICENSE.md'),
    'notice'
  )
  writeFileSync(
    path.join(
      project,
      'app/static/common/choosealicense.com/_licenses/example.txt'
    ),
    '---\ntitle: Example\nhidden: false\n---\nExample license\n'
  )
  writeFileSync(path.join(project, 'app/static/common/logo.svg'), 'before')
  return { project, renderer, output }
}

describe('Rust development Vite watcher', () => {
  it('rebuilds changed source without removing the adjacent worker bundle', async () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'desktop-rust-watch-'))
    const output = path.join(directory, 'out')
    const source = path.join(directory, 'entry.ts')
    const config = path.join(directory, 'vite.config.ts')
    mkdirSync(output)
    writeFileSync(path.join(output, 'highlighter.js'), 'worker')
    writeFileSync(source, "export const marker = 'before-change'\n")
    writeFileSync(
      config,
      `export default {
      root: ${JSON.stringify(directory)},
      logLevel: 'silent',
      build: { outDir: ${JSON.stringify(output)}, minify: false,
        lib: { entry: ${JSON.stringify(
          source
        )}, formats: ['es'], fileName: () => 'bundle.js' } }
    }`
    )
    let watcher: DevWatcher | undefined
    try {
      const bundle = await watchRustBundle(config)
      watcher = bundle.watcher
      await bounded(bundle.ready, 15_000)
      assert.match(
        readFileSync(path.join(output, 'bundle.js'), 'utf8'),
        /before-change/
      )
      const rebuilt = new Promise<void>((resolve, reject) => {
        bundle.watcher.on('event', event => {
          if (event.code !== 'END' && event.code !== 'ERROR') {
            return
          }
          if (event.code === 'ERROR') {
            reject(event.error)
          } else {
            resolve()
          }
        })
      })
      writeFileSync(source, "export const marker = 'after-change'\n")
      await bounded(rebuilt, 15_000)
      const result = readFileSync(path.join(output, 'bundle.js'), 'utf8')
      assert.match(result, /after-change/)
      assert.doesNotMatch(result, /before-change/)
      assert.equal(
        readFileSync(path.join(output, 'highlighter.js'), 'utf8'),
        'worker'
      )
    } finally {
      if (watcher) {
        await watcher.close()
      }
      rmSync(directory, { recursive: true, force: true })
    }
  }, 30_000)

  it('waits for static resources and rebuilds when they change without deleting foreign files', async () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'desktop-rust-static-'))
    const { project, renderer, output } = staticFixture(directory)
    const foreign = path.join(directory, 'foreign.txt')
    const unmanaged = path.join(output, 'unmanaged.txt')
    writeFileSync(foreign, 'keep')
    writeFileSync(unmanaged, 'unmanaged')
    let allowCopy = () => {}
    let reportStarted = () => {}
    const gate = new Promise<void>(resolve => {
      allowCopy = resolve
    })
    const started = new Promise<void>(resolve => {
      reportStarted = resolve
    })
    const licenseDump = async () => {
      reportStarted()
      await gate
      return {}
    }
    let watcher: DevWatcher | undefined
    try {
      const bundle = await watchRustBundle(false, {
        root: renderer,
        logLevel: 'silent',
        plugins: [copyStaticResources('linux', project, licenseDump)],
        build: {
          outDir: output,
          minify: false,
          lib: {
            entry: path.join(renderer, 'entry.ts'),
            formats: ['es'],
            fileName: () => 'bundle.js',
          },
        },
      })
      watcher = bundle.watcher
      let settled = false
      void bundle.ready.then(
        () => {
          settled = true
        },
        () => {
          settled = true
        }
      )
      await bounded(started, 15_000)
      await new Promise(resolve => setTimeout(resolve, 100))
      assert.equal(
        settled,
        false,
        'watcher became ready before the copy finished'
      )
      allowCopy()
      await bounded(bundle.ready, 15_000)
      const logo = path.join(project, 'app/static/common/logo.svg')
      const copiedLogo = path.join(output, 'static/logo.svg')
      assert.equal(readFileSync(copiedLogo, 'utf8'), 'before')
      const updated = nextBuild(watcher)
      writeFileSync(logo, 'after')
      await bounded(updated, 15_000)
      assert.equal(readFileSync(copiedLogo, 'utf8'), 'after')
      const emoji = path.join(project, 'gemoji/images/emoji/test.png')
      const emojiUpdated = nextBuild(watcher)
      writeFileSync(emoji, 'emoji-after')
      await bounded(emojiUpdated, 15_000)
      assert.equal(
        readFileSync(path.join(output, 'emoji/test.png'), 'utf8'),
        'emoji-after'
      )
      const template = path.join(
        project,
        'app/static/common/gitignore/Example.gitignore'
      )
      const templateUpdated = nextBuild(watcher)
      writeFileSync(template, 'updated-ignore')
      await bounded(templateUpdated, 15_000)
      assert.equal(
        readFileSync(
          path.join(output, 'static/gitignore/Example.gitignore'),
          'utf8'
        ),
        'updated-ignore'
      )
      if (process.platform !== 'win32') {
        const linked = path.join(
          project,
          'app/static/common/gitignore/linked-template'
        )
        const linkedUpdated = nextBuild(watcher)
        writeFileSync(linked, 'linked-after')
        await bounded(linkedUpdated, 15_000)
        assert.equal(
          readFileSync(
            path.join(output, 'static/gitignore/Linked.gitignore'),
            'utf8'
          ),
          'linked-after'
        )
      }
      const license = path.join(
        project,
        'app/static/common/choosealicense.com/_licenses/example.txt'
      )
      const licenseUpdated = nextBuild(watcher)
      writeFileSync(
        license,
        '---\ntitle: Example\nhidden: false\n---\nUpdated license\n'
      )
      await bounded(licenseUpdated, 15_000)
      assert.match(
        readFileSync(
          path.join(output, 'static/available-licenses.json'),
          'utf8'
        ),
        /Updated license/
      )
      const extra = path.join(
        project,
        'app/static/common/choosealicense.com/_licenses/extra.txt'
      )
      const licenseAdded = nextBuild(watcher)
      writeFileSync(
        extra,
        '---\ntitle: Extra\nhidden: false\n---\nAdded license\n'
      )
      await bounded(licenseAdded, 15_000)
      assert.match(
        readFileSync(
          path.join(output, 'static/available-licenses.json'),
          'utf8'
        ),
        /Added license/
      )
      const licenseRemoved = nextBuild(watcher)
      rmSync(extra)
      await bounded(licenseRemoved, 15_000)
      assert.doesNotMatch(
        readFileSync(
          path.join(output, 'static/available-licenses.json'),
          'utf8'
        ),
        /Added license/
      )
      const link = path.join(output, 'static/foreign-link')
      if (process.platform !== 'win32') {
        symlinkSync(foreign, link)
      }
      if (process.platform !== 'win32') {
        rmSync(copiedLogo)
        symlinkSync(foreign, copiedLogo)
      }
      const previousBundle = readFileSync(
        path.join(output, 'bundle.js'),
        'utf8'
      )
      const removed = nextBuild(watcher)
      rmSync(logo)
      await assert.rejects(
        bounded(removed, 15_000),
        /static resource was removed/
      )
      assert.equal(
        readFileSync(path.join(output, 'bundle.js'), 'utf8'),
        previousBundle
      )
      assert.equal(
        readFileSync(copiedLogo, 'utf8'),
        process.platform === 'win32' ? 'after' : 'keep'
      )
      assert.equal(readFileSync(foreign, 'utf8'), 'keep')
      assert.equal(readFileSync(unmanaged, 'utf8'), 'unmanaged')
      if (process.platform !== 'win32') {
        assert.equal(readFileSync(link, 'utf8'), 'keep')
      }
    } finally {
      allowCopy()
      if (watcher) {
        await watcher.close()
      }
      rmSync(directory, { recursive: true, force: true })
    }
  }, 60_000)

  it('rejects startup when copying static resources fails', async () => {
    const directory = mkdtempSync(
      path.join(tmpdir(), 'desktop-rust-static-fail-')
    )
    const { project, renderer, output } = staticFixture(directory)
    let watcher: DevWatcher | undefined
    try {
      const bundle = await watchRustBundle(false, {
        root: renderer,
        logLevel: 'silent',
        plugins: [
          copyStaticResources('linux', project, async () => {
            throw new Error('injected license failure')
          }),
        ],
        build: {
          outDir: output,
          minify: false,
          lib: {
            entry: path.join(renderer, 'entry.ts'),
            formats: ['es'],
            fileName: () => 'bundle.js',
          },
        },
      })
      watcher = bundle.watcher
      await assert.rejects(
        bounded(bundle.ready, 15_000),
        /injected license failure/
      )
    } finally {
      if (watcher) {
        await watcher.close()
      }
      rmSync(directory, { recursive: true, force: true })
    }
  }, 30_000)

  it('rejects removal of the emoji database before publishing a new renderer', async () => {
    const directory = mkdtempSync(
      path.join(tmpdir(), 'desktop-rust-static-missing-')
    )
    const { project, renderer, output } = staticFixture(directory)
    let watcher: DevWatcher | undefined
    try {
      const bundle = await watchRustBundle(false, {
        root: renderer,
        logLevel: 'silent',
        plugins: [copyStaticResources('linux', project, async () => ({}))],
        build: {
          outDir: output,
          minify: false,
          lib: {
            entry: path.join(renderer, 'entry.ts'),
            formats: ['es'],
            fileName: () => 'bundle.js',
          },
        },
      })
      watcher = bundle.watcher
      await bounded(bundle.ready, 15_000)
      const previous = readFileSync(path.join(output, 'bundle.js'), 'utf8')
      const failed = nextBuild(watcher)
      rmSync(path.join(project, 'gemoji/db/emoji.json'))
      await assert.rejects(
        bounded(failed, 15_000),
        /static resource was removed/
      )
      assert.equal(
        readFileSync(path.join(output, 'bundle.js'), 'utf8'),
        previous
      )
      assert.equal(readFileSync(path.join(output, 'emoji.json'), 'utf8'), '{}')
    } finally {
      if (watcher) {
        await watcher.close()
      }
      rmSync(directory, { recursive: true, force: true })
    }
  }, 30_000)
})

describe.skipIf(process.platform === 'win32')('Rust build cancellation', () => {
  it('stops the build process group even when a child ignores interrupts', async () => {
    const directory = mkdtempSync(
      path.join(tmpdir(), 'desktop-rust-build-stop-')
    )
    const parent = path.join(directory, 'parent.cjs')
    const worker = path.join(directory, 'worker.cjs')
    const ready = path.join(directory, 'ready')
    const release = path.join(directory, 'release')
    const late = path.join(directory, 'late')
    writeFileSync(
      worker,
      `
      const fs = require('node:fs')
      process.on('SIGINT', () => {})
      process.on('SIGTERM', () => {})
      fs.writeFileSync(${JSON.stringify(ready)}, 'ready')
      setInterval(() => {
        if (fs.existsSync(${JSON.stringify(
          release
        )})) fs.writeFileSync(${JSON.stringify(late)}, 'late')
      }, 20)
    `
    )
    writeFileSync(
      parent,
      `
      const { spawn } = require('node:child_process')
      process.on('SIGINT', () => {})
      process.on('SIGTERM', () => {})
      spawn(process.execPath, [${JSON.stringify(worker)}], { stdio: 'ignore' })
      setInterval(() => {}, 1000)
    `
    )
    const child = spawn(process.execPath, [parent], {
      detached: true,
      stdio: 'ignore',
    })
    const closed = new Promise<void>((resolve, reject) => {
      child.once('error', reject)
      child.once('close', () => resolve())
    })
    try {
      const deadline = Date.now() + 5_000
      while (!existsSync(ready) && Date.now() < deadline) {
        await new Promise(resolve => setTimeout(resolve, 20))
      }
      assert.ok(existsSync(ready), 'build child did not start')
      await bounded(stopDevelopmentBuild(child), 6_000)
      await bounded(closed, 5_000)
      writeFileSync(release, 'release')
      await new Promise(resolve => setTimeout(resolve, 300))
      assert.equal(existsSync(late), false)
    } finally {
      if (child.pid && child.exitCode === null && child.signalCode === null) {
        try {
          process.kill(-child.pid, 'SIGKILL')
        } catch {}
      }
      await bounded(closed, 5_000).catch(() => {})
      rmSync(directory, { recursive: true, force: true })
    }
  }, 20_000)
})

describe.skipIf(process.platform === 'win32')(
  'Rust development shutdown signals',
  () => {
    it('keeps the supervisor alive through repeated Ctrl-C, SIGTERM and SIGHUP while draining', async () => {
      const directory = mkdtempSync(
        path.join(tmpdir(), 'desktop-rust-signals-')
      )
      const script = path.join(directory, 'signals.ts')
      const started = path.join(directory, 'started')
      const result = path.join(directory, 'result')
      writeFileSync(
        script,
        `
      import { writeFileSync } from 'node:fs'
      import { installDevelopmentStopSignals } from ${JSON.stringify(
        path.resolve('script/rust-dev-signals.ts')
      )}
      let count = 0
      installDevelopmentStopSignals(() => {
        count++
        writeFileSync(${JSON.stringify(started)}, 'yes')
        setTimeout(() => { writeFileSync(${JSON.stringify(
          result
        )}, String(count)); process.exit(0) }, 800)
      })
      console.log('READY')
      setInterval(() => {}, 1000)
    `
      )
      const child = spawn(process.execPath, ['--import', 'tsx', script], {
        cwd: process.cwd(),
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      let output = ''
      child.stdout.on('data', chunk => {
        output += String(chunk)
      })
      child.stderr.on('data', chunk => {
        output += String(chunk)
      })
      const closed = new Promise<number | null>((resolve, reject) => {
        child.once('error', reject)
        child.once('close', resolve)
      })
      try {
        await bounded(
          waitForMessage(child, () => output, /READY/),
          10_000
        )
        assert.equal(child.kill('SIGINT'), true)
        const deadline = Date.now() + 5_000
        while (!existsSync(started) && Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 20))
        }
        assert.ok(existsSync(started), output)
        for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP'] as const) {
          assert.equal(child.kill(signal), true)
          await new Promise(resolve => setTimeout(resolve, 50))
        }
        assert.equal(await bounded(closed, 5_000), 0, output)
        assert.equal(readFileSync(result, 'utf8'), '1')
      } finally {
        if (child.exitCode === null && child.signalCode === null) {
          child.kill('SIGKILL')
        }
        await bounded(closed, 5_000).catch(() => {})
        rmSync(directory, { recursive: true, force: true })
      }
    }, 20_000)
  }
)

async function waitForMessage(
  child: ChildProcess,
  getOutput: () => string,
  message: RegExp
) {
  const deadline = Date.now() + 180_000
  while (!message.test(getOutput())) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(
        `Launcher exited before startup: ${getOutput().slice(-5_000)}`
      )
    }
    if (Date.now() >= deadline) {
      throw new Error(`Launcher did not start: ${getOutput().slice(-5_000)}`)
    }
    await new Promise(resolve => setTimeout(resolve, 100))
  }
}

/** The host is detached from the supervisor's process group but remains its child. */
function hostPid(supervisorPid: number): number {
  const children = readFileSync(
    `/proc/${supervisorPid}/task/${supervisorPid}/children`,
    'utf8'
  )
    .trim()
    .split(/\s+/)
    .filter(Boolean)
  for (const child of children) {
    try {
      if (
        path
          .basename(readlinkSync(`/proc/${child}/exe`))
          .replace(/ \(deleted\)$/, '') === 'github-desktop-rs'
      ) {
        return Number(child)
      }
    } catch {
      // Short-lived helper processes may disappear while inspecting /proc.
    }
  }
  throw new Error('The development supervisor did not start the Rust host')
}

describe.skipIf(!enabled)('Rust and Vite development launcher', () => {
  it('mounts the real renderer and stops its watchers when the window exits', async () => {
    const run = startHost(['--smoke-test'])
    try {
      const result = await bounded(run.closed, 240_000)
      assert.equal(result.code, 0, run.output().slice(-5_000))
      assert.match(
        run.output(),
        /PASS: native webview mounted React and completed shell and backend RPC/
      )
    } finally {
      if (run.child.exitCode === null && run.child.signalCode === null) {
        run.child.kill('SIGTERM')
      }
      await bounded(run.closed, 15_000).catch(() => {})
      rmSync(run.directory, { recursive: true, force: true })
    }
  }, 260_000)

  it('requests a drained native shutdown on Ctrl-C and leaves no host process', async () => {
    const run = startHost()
    let nativePid: number | undefined
    try {
      await waitForMessage(
        run.child,
        run.output,
        /webview ready; entering tao event loop/
      )
      assert.ok(run.child.pid)
      nativePid = hostPid(run.child.pid)
      assert.equal(run.child.kill('SIGINT'), true)
      const result = await bounded(run.closed, 20_000)
      assert.equal(result.code, 0, run.output().slice(-5_000))
      assert.match(run.output(), /desktop-dev-shutdown:drained/)
      assert.equal(existsSync(`/proc/${nativePid}`), false)
    } finally {
      if (run.child.exitCode === null && run.child.signalCode === null) {
        run.child.kill('SIGTERM')
      }
      await bounded(run.closed, 15_000).catch(() => {})
      if (nativePid && existsSync(`/proc/${nativePid}`)) {
        try {
          process.kill(nativePid, 'SIGKILL')
        } catch {}
      }
      rmSync(run.directory, { recursive: true, force: true })
    }
  }, 260_000)
})
