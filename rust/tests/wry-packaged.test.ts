// @vitest-environment node
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'vitest'

/** A relocated package must not retain a link into the original checkout. */
function assertLinksStayWithinPackage(root: string, entry = root): void {
  const stat = lstatSync(entry)
  if (stat.isSymbolicLink()) {
    const target = realpathSync(entry)
    const relative = path.relative(root, target)
    assert.ok(
      relative !== '..' &&
        !relative.startsWith(`..${path.sep}`) &&
        !path.isAbsolute(relative),
      `Package link escapes relocation: ${entry}`
    )
  } else if (stat.isDirectory()) {
    for (const name of readdirSync(entry)) {
      assertLinksStayWithinPackage(root, path.join(entry, name))
    }
  }
}

describe.skipIf(process.platform !== 'linux')(
  'relocated package link checks',
  () => {
    it('allows internal links and rejects links to files outside the package', () => {
      const temporary = mkdtempSync(
        path.join(tmpdir(), 'desktop-packaged-links-')
      )
      try {
        const root = path.join(temporary, 'app')
        mkdirSync(root)
        writeFileSync(path.join(root, 'inside'), 'ok')
        writeFileSync(path.join(temporary, 'outside'), 'external')
        symlinkSync('inside', path.join(root, 'internal'))
        assert.doesNotThrow(() =>
          assertLinksStayWithinPackage(realpathSync(root))
        )
        symlinkSync(
          path.join(temporary, 'outside'),
          path.join(root, 'external')
        )
        assert.throws(
          () => assertLinksStayWithinPackage(realpathSync(root)),
          /escapes relocation/
        )
      } finally {
        rmSync(temporary, { recursive: true, force: true })
      }
    })
  }
)

/** Exercise a real package without changing the checkout or the user's profile. */
describe.skipIf(
  process.platform !== 'linux' || process.env.DESKTOP_WRY_RELOCATED_TEST !== '1'
)('relocated Rust desktop smoke test', () => {
  it('opens the packaged renderer and backend from a relocated directory', async () => {
    const source = process.env.DESKTOP_RUST_PACKAGE
    assert.ok(
      source && path.isAbsolute(source),
      'Set DESKTOP_RUST_PACKAGE to an absolute package path'
    )
    assert.ok(
      lstatSync(source).isDirectory(),
      'The package root must be a real directory, not a symlink'
    )
    assert.ok(
      existsSync(path.join(source, 'bin/github-desktop-rs')),
      'Build the Rust package first'
    )
    const temporary = mkdtempSync(
      path.join(tmpdir(), 'desktop-packaged-smoke-')
    )
    const relocated = path.join(temporary, 'relocated app')
    let child: ReturnType<typeof spawn> | undefined
    let safeToDelete = true
    try {
      cpSync(source, relocated, { recursive: true, verbatimSymlinks: true })
      assertLinksStayWithinPackage(realpathSync(relocated))
      for (const directory of [
        'user',
        'config',
        'data',
        'cache',
        'runtime',
        'config-dirs',
        'data-dirs',
      ]) {
        mkdirSync(path.join(temporary, directory), { mode: 0o700 })
      }
      const env: NodeJS.ProcessEnv = Object.fromEntries(
        Object.entries(process.env).filter(
          ([key]) =>
            !key.startsWith('GIT_') &&
            !key.startsWith('DESKTOP_') &&
            ![
              'GH_TOKEN',
              'GITHUB_TOKEN',
              'SSH_AUTH_SOCK',
              'SSH_AGENT_PID',
              'GNOME_KEYRING_CONTROL',
              'DBUS_SESSION_BUS_ADDRESS',
              'DBUS_STARTER_ADDRESS',
              'DBUS_STARTER_BUS_TYPE',
              'WAYLAND_DISPLAY',
            ].includes(key)
        )
      )
      Object.assign(env, {
        DESKTOP_USER_DATA: path.join(temporary, 'user'),
        // Packaged UI must ignore the development-only disk override.
        DESKTOP_SERVE_FROM_DISK: '1',
        XDG_CONFIG_HOME: path.join(temporary, 'config'),
        XDG_DATA_HOME: path.join(temporary, 'data'),
        XDG_CACHE_HOME: path.join(temporary, 'cache'),
        XDG_RUNTIME_DIR: path.join(temporary, 'runtime'),
        XDG_CONFIG_DIRS: path.join(temporary, 'config-dirs'),
        XDG_DATA_DIRS: path.join(temporary, 'data-dirs'),
        DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(
          temporary,
          'no-dbus'
        )}`,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(temporary, 'gitconfig'),
        GDK_BACKEND: 'x11',
        WEBKIT_DISABLE_COMPOSITING_MODE: '1',
        WEBKIT_DISABLE_DMABUF_RENDERER: '1',
        LIBGL_ALWAYS_SOFTWARE: '1',
        NO_AT_BRIDGE: '1',
      })
      child = spawn(
        'xvfb-run',
        ['-a', path.join(relocated, 'bin/github-desktop-rs'), '--smoke-test'],
        {
          env,
          cwd: temporary,
          detached: true,
          stdio: ['ignore', 'pipe', 'pipe'],
        }
      )
      let output = ''
      const collect = (data: Buffer) => {
        output = (output + data.toString()).slice(-32_768)
      }
      child.stdout?.on('data', collect)
      child.stderr?.on('data', collect)
      safeToDelete = child.pid === undefined
      const running = child
      const signal = (name: NodeJS.Signals | 0) => {
        if (running.pid === undefined) return false
        try {
          process.kill(-running.pid, name)
          return true
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false
          throw error
        }
      }
      const result = await new Promise<{
        code: number | null
        signal: NodeJS.Signals | null
      }>((resolve, reject) => {
        const timeout = setTimeout(() => signal('SIGKILL'), 60_000)
        const hardTimeout = setTimeout(
          () =>
            reject(
              new Error(
                'Packaged desktop did not exit; temporary files retained'
              )
            ),
          70_000
        )
        running.once('error', error => {
          clearTimeout(timeout)
          clearTimeout(hardTimeout)
          reject(error)
        })
        running.once('close', (code, signal) => {
          clearTimeout(timeout)
          clearTimeout(hardTimeout)
          resolve({ code, signal })
        })
      })
      // xvfb-run normally reaps its server. Do not delete files if a child
      // process remains after it exits.
      if (signal(0)) {
        signal('SIGTERM')
        const deadline = Date.now() + 5_000
        while (signal(0) && Date.now() < deadline) {
          await new Promise(resolve => setTimeout(resolve, 50))
        }
        if (signal(0)) {
          signal('SIGKILL')
          throw new Error(
            'Packaged desktop process group did not exit; temporary files retained'
          )
        }
      }
      safeToDelete = true
      assert.equal(
        result.code,
        0,
        `packaged shell failed: ${JSON.stringify(result)}\n${output}`
      )
      assert.equal(
        result.signal,
        null,
        `packaged shell was signalled: ${output}`
      )
      assert.match(
        output,
        /PASS: native webview mounted React and completed shell and backend RPC/
      )
    } finally {
      if (safeToDelete) rmSync(temporary, { recursive: true, force: true })
      else console.error(`Retained packaged desktop fixture: ${temporary}`)
    }
  }, 100_000)
})

// Requires a Rust package, Linux user/mount namespaces, WebKitGTK and Xvfb.
describe.skipIf(
  process.platform !== 'linux' || process.env.DESKTOP_WRY_PACKAGE_TEST !== '1'
)('relocated Rust package', () => {
  it('loads the embedded renderer while the build checkout has different assets', async () => {
    const source = process.env.DESKTOP_RUST_PACKAGE
    assert.ok(
      source && path.isAbsolute(source),
      'Set DESKTOP_RUST_PACKAGE to an absolute package path'
    )
    const binary = path.join(source, 'bin/github-desktop-rs')
    const checkout = path.resolve('out/vite-dist')
    assert.ok(existsSync(binary), 'Build the Rust package first')
    assert.ok(
      existsSync(checkout),
      'The checkout bundle is needed as a mount target'
    )

    const temporary = mkdtempSync(
      path.join(tmpdir(), 'desktop-relocated-package-')
    )
    const home = path.join(temporary, 'home')
    const poison = path.join(temporary, 'unrelated-checkout-bundle')
    const relocated = path.join(
      temporary,
      'relocated app/bin/github-desktop-rs'
    )
    mkdirSync(path.dirname(relocated), { recursive: true })
    mkdirSync(poison)
    for (const item of ['config', 'data', 'cache', 'runtime']) {
      mkdirSync(path.join(home, item), { recursive: true, mode: 0o700 })
    }
    cpSync(binary, relocated)
    // If the packaged debug shell reads this checkout, the real React mount
    // never appears and its built-in smoke test will fail.
    writeFileSync(
      path.join(poison, 'index.html'),
      '<!doctype html><title>wrong renderer</title>'
    )
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: home }
    for (const key of Object.keys(env)) {
      if (key.startsWith('GIT_')) {
        delete env[key]
      }
    }
    // A packaged shell must ignore this development-only override as well.
    env.DESKTOP_SERVE_FROM_DISK = '1'
    delete env.WAYLAND_DISPLAY
    Object.assign(env, {
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(temporary, 'gitconfig'),
      DESKTOP_USER_DATA: path.join(home, 'user-data'),
      XDG_CONFIG_HOME: path.join(home, 'config'),
      XDG_DATA_HOME: path.join(home, 'data'),
      XDG_CACHE_HOME: path.join(home, 'cache'),
      XDG_RUNTIME_DIR: path.join(home, 'runtime'),
      DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(temporary, 'no-dbus')}`,
      GDK_BACKEND: 'x11',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      NO_AT_BRIDGE: '1',
    })

    let safeToDelete = true
    try {
      const child = spawn(
        'unshare',
        [
          '--user',
          '--map-root-user',
          '--mount',
          '--propagation',
          'private',
          'sh',
          '-c',
          'mount --bind "$1" "$2" || exit; exec xvfb-run -a "$3" --smoke-test',
          'sh',
          poison,
          checkout,
          relocated,
        ],
        { env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] }
      )
      safeToDelete = false
      let output = ''
      const collect = (data: Buffer) => {
        output = (output + data.toString()).slice(-32_768)
      }
      child.stdout.on('data', collect)
      child.stderr.on('data', collect)
      const result = await new Promise<{
        code: number | null
        signal: NodeJS.Signals | null
      }>((resolve, reject) => {
        const timer = setTimeout(() => {
          if (child.pid !== undefined) {
            try {
              process.kill(-child.pid, 'SIGKILL')
            } catch {
              /* already exited */
            }
          }
        }, 75_000)
        child.once('error', error => {
          clearTimeout(timer)
          if (child.pid === undefined) {
            safeToDelete = true
          }
          reject(error)
        })
        child.once('close', (code, signal) => {
          clearTimeout(timer)
          resolve({ code, signal })
        })
      })
      safeToDelete = true
      assert.equal(
        result.code,
        0,
        `packaged shell failed: ${JSON.stringify(result)}\n${output}`
      )
      assert.match(
        output,
        /PASS: native webview mounted React and completed shell and backend RPC/
      )
    } finally {
      if (safeToDelete) {
        rmSync(temporary, { recursive: true, force: true })
      }
    }
  }, 90_000)

  it('uses only an adjacent or explicitly configured Copilot runtime', () => {
    const source = process.env.DESKTOP_RUST_PACKAGE
    assert.ok(source && path.isAbsolute(source))
    const binary = path.join(source, 'bin/desktop-rs')
    assert.ok(existsSync(binary), 'Build the Rust package first')
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-package-copilot-'))
    try {
      const bin = path.join(root, 'relocated app/bin')
      const repo = path.join(root, 'untrusted repo')
      mkdirSync(bin, { recursive: true })
      mkdirSync(repo, { recursive: true })
      const executable = path.join(bin, 'desktop-rs')
      cpSync(binary, executable)
      const env: NodeJS.ProcessEnv = { ...process.env, HOME: root }
      for (const key of Object.keys(env)) {
        if (key.startsWith('GIT_')) {
          delete env[key]
        }
      }
      Object.assign(env, {
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
        COPILOT_RUNTIME_ROOT: '',
      })
      execFileSync('git', ['-c', 'init.templateDir=', 'init', '-q', repo], {
        env,
      })
      const report = process.report?.getReport() as
        | { header?: { glibcVersionRuntime?: string } }
        | undefined
      const platform =
        report && !report.header?.glibcVersionRuntime ? 'linuxmusl' : 'linux'
      const runtimeDirectory = `prebuilds/${platform}-${process.arch}`
      const packageName = `copilot-sdk-${platform}-${process.arch}`
      const app = JSON.parse(
        readFileSync(path.resolve('app/package.json'), 'utf8')
      ) as { dependencies: Record<string, string> }
      const untrusted = path.join(repo, 'node_modules/@github', packageName)
      for (const location of [untrusted, repo]) {
        mkdirSync(path.join(location, runtimeDirectory), { recursive: true })
        writeFileSync(
          path.join(location, runtimeDirectory, 'copilot-runtime'),
          'not executable'
        )
      }
      writeFileSync(
        path.join(untrusted, 'package.json'),
        JSON.stringify({ version: app.dependencies['@github/copilot-sdk'] })
      )
      const probe = () =>
        JSON.parse(
          execFileSync(
            executable,
            ['-C', repo, 'repo', '{"method":"copilot_probe"}'],
            { cwd: repo, env, encoding: 'utf8', timeout: 20_000 }
          )
        ) as { available: boolean; reason: string }
      assert.equal(probe().available, false, 'untrusted working tree was used')
      const adjacent = path.join(bin, 'copilot-sdk')
      mkdirSync(path.join(adjacent, runtimeDirectory), { recursive: true })
      writeFileSync(
        path.join(adjacent, runtimeDirectory, 'copilot-runtime'),
        'adjacent fixture'
      )
      const bundled = probe()
      assert.equal(bundled.available, true)
      assert.ok(bundled.reason.includes(adjacent))
      const explicit = path.join(root, 'explicit')
      mkdirSync(path.join(explicit, runtimeDirectory), { recursive: true })
      writeFileSync(
        path.join(explicit, runtimeDirectory, 'copilot-runtime'),
        'explicit fixture'
      )
      env.COPILOT_RUNTIME_ROOT = explicit
      const overridden = probe()
      assert.equal(overridden.available, true)
      assert.ok(overridden.reason.includes(explicit))
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  }, 90_000)
})
