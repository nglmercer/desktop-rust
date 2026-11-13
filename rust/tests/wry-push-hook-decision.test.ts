// @vitest-environment node
import assert from 'node:assert/strict'
import { execFileSync, spawn } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { build, mergeConfig } from 'vite'
import { describe, it } from 'vitest'
import rendererConfig from '../../vite.config'

async function stopProcessGroup(pid: number, closed: Promise<void>) {
  const signal = (name: NodeJS.Signals) => {
    try {
      process.kill(-pid, name)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') {
        throw error
      }
    }
  }
  const wait = async () => {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        closed.then(() => true),
        new Promise<false>(resolve => {
          timer = setTimeout(() => resolve(false), 5_000)
        }),
      ])
    } finally {
      if (timer !== undefined) {
        clearTimeout(timer)
      }
    }
  }
  signal('SIGTERM')
  if (!(await wait())) {
    signal('SIGKILL')
    if (!(await wait())) {
      throw new Error('Wry process group did not close')
    }
  }
}

// Opt-in: requires Linux user/mount namespaces, Xvfb, WebKitGTK and a debug shell.
describe.skipIf(
  process.env.DESKTOP_WRY_WEBVIEW_TEST !== '1' || process.platform !== 'linux'
)('Wry interactive pre-push hooks', () => {
  it('passes approval, rejection and cancellation through the real webview', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-wry-push-hook-'))
    const home = path.join(root, 'home')
    const output = path.join(root, 'bundle')
    const entry = path.join(root, 'probe.ts')
    const binary = path.resolve('rust/target/debug/github-desktop-rs')
    const mountpoint = path.resolve('out/vite-dist')
    const env: NodeJS.ProcessEnv = { ...process.env, HOME: home }
    for (const key of Object.keys(env)) {
      if (key.startsWith('GIT_')) {
        delete env[key]
      }
    }
    delete env.WAYLAND_DISPLAY
    Object.assign(env, {
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
      GIT_TERMINAL_PROMPT: '0',
      DESKTOP_USER_DATA: path.join(home, 'user-data'),
      XDG_CONFIG_HOME: path.join(home, 'config'),
      XDG_DATA_HOME: path.join(home, 'data'),
      XDG_CACHE_HOME: path.join(home, 'cache'),
      XDG_RUNTIME_DIR: path.join(home, 'runtime'),
      DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(root, 'no-dbus')}`,
      GDK_BACKEND: 'x11',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      NO_AT_BRIDGE: '1',
    })
    const git = (...args: string[]) =>
      execFileSync('git', args, {
        env,
        encoding: 'utf8',
        timeout: 20_000,
        stdio: ['ignore', 'pipe', 'pipe'],
      }).trim()
    let safeToDelete = true
    try {
      assert.ok(existsSync(binary), 'Build the debug desktop-app binary first')
      assert.ok(existsSync(mountpoint), 'Build the web renderer first')
      for (const directory of ['config', 'data', 'cache', 'runtime']) {
        mkdirSync(path.join(home, directory), { recursive: true, mode: 0o700 })
      }
      const fixtures = ['approve', 'reject', 'cancel'].map(mode => {
        const repository = path.join(root, `repo-${mode}`)
        const remote = path.join(root, `remote-${mode}.git`)
        mkdirSync(repository)
        git('init', '-q', '-b', 'main', repository)
        git('init', '--bare', '-q', remote)
        git('-C', repository, 'config', 'user.name', 'Test')
        git('-C', repository, 'config', 'user.email', 'test@example.invalid')
        writeFileSync(path.join(repository, 'file'), mode)
        git('-C', repository, 'add', 'file')
        git('-C', repository, 'commit', '-qm', mode)
        git('-C', repository, 'remote', 'add', 'origin', remote)
        const hook = path.join(repository, '.git/hooks/pre-push')
        writeFileSync(hook, '#!/bin/sh\necho tested-hook-failure >&2\nexit 1\n')
        chmodSync(hook, 0o755)
        return { mode, repository, remote }
      })
      const client = path.resolve('app/src/backend/client')
      writeFileSync(
        entry,
        `
import { BackendClient } from ${JSON.stringify(client)}
;(window as any).runProbe = async () => {
  const client = new BackendClient()
  const outcomes: string[] = []
  for (const fixture of ${JSON.stringify(fixtures)}) {
    const controller = new AbortController()
    let calls = 0
    try {
      const result = await client.pushRepositoryRemote(fixture.repository, {
        remote: 'origin', expected_url: fixture.remote, local_branch: 'main',
        remote_branch: null, tags: [], force_with_lease: false, no_verify: false,
        report_progress: false, require_hook_callbacks: true, interactive_hook_decision: true,
      }, undefined, undefined, undefined, controller.signal, async output => {
        calls++
        if (!new TextDecoder().decode(output).includes('tested-hook-failure')) throw new Error('missing hook output')
        if (fixture.mode === 'cancel') {
          controller.abort()
          return await new Promise<'abort' | 'ignore'>(() => {})
        }
        return fixture.mode === 'approve' ? 'ignore' : 'abort'
      })
      if (fixture.mode === 'approve' && result !== undefined) throw new Error('approval did not publish')
      if (fixture.mode === 'reject' && (!result || result.exitCode === 0)) throw new Error('rejection was accepted')
      if (fixture.mode === 'cancel') throw new Error('cancelled push succeeded')
    } catch (error) {
      if (fixture.mode !== 'cancel') throw error
      if (!(error instanceof Error) || (error as Error & { code?: string }).code !== 'push_outcome_uncertain') throw error
    }
    if (calls !== 1) throw new Error('expected one hook decision for ' + fixture.mode + ', got ' + calls)
    outcomes.push(fixture.mode)
  }
  await new Promise(resolve => setTimeout(resolve, 500))
  return outcomes
}
`
      )
      await build(
        mergeConfig(rendererConfig, {
          logLevel: 'silent',
          build: {
            outDir: output,
            emptyOutDir: true,
            rollupOptions: { input: entry },
            lib: {
              entry,
              formats: ['iife'],
              name: 'WryPushProbe',
              fileName: () => 'probe.js',
            },
          },
        })
      )
      writeFileSync(
        path.join(output, 'index.html'),
        `<!doctype html><meta charset="UTF-8"><script src="./probe.js"></script><script>window.runProbe().then(value => console.error('WRY_PUSH_SUCCESS ' + JSON.stringify(value)), error => console.error('WRY_PUSH_FAILURE ' + JSON.stringify({ message: String(error), stack: String(error && error.stack), code: error && error.code })))</script>`
      )
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
          'mount --bind "$1" "$2" || exit; exec xvfb-run -a "$3"',
          'sh',
          output,
          mountpoint,
          binary,
        ],
        { detached: true, env, stdio: ['ignore', 'pipe', 'pipe'] }
      )
      safeToDelete = false
      const closed = new Promise<void>(resolve =>
        child.once('close', () => resolve())
      )
      try {
        const message = await new Promise<string>((resolve, reject) => {
          let text = ''
          const timeout = setTimeout(
            () => reject(new Error(`Wry probe timed out: ${text}`)),
            60_000
          )
          const collect = (chunk: Buffer) => {
            text = (text + chunk.toString()).slice(-32_768)
            const result = text.match(/WRY_PUSH_(SUCCESS|FAILURE) ([^\r\n]*)/)
            if (result) {
              clearTimeout(timeout)
              if (result[1] === 'SUCCESS') {
                resolve(result[2])
              } else {
                reject(new Error(`Wry push probe failed: ${result[2]}`))
              }
            }
          }
          child.stdout.on('data', collect)
          child.stderr.on('data', collect)
          child.once('error', error => {
            clearTimeout(timeout)
            reject(error)
          })
          child.once('exit', code => {
            clearTimeout(timeout)
            reject(new Error(`Wry exited ${code}: ${text}`))
          })
        })
        assert.deepEqual(JSON.parse(message), ['approve', 'reject', 'cancel'])
        assert.equal(
          git('-C', fixtures[0].remote, 'rev-parse', 'refs/heads/main'),
          git('-C', fixtures[0].repository, 'rev-parse', 'HEAD')
        )
        for (const fixture of fixtures.slice(1)) {
          assert.throws(() =>
            git(
              '-C',
              fixture.remote,
              'rev-parse',
              '--verify',
              'refs/heads/main'
            )
          )
        }
      } finally {
        if (child.pid !== undefined) {
          await stopProcessGroup(child.pid, closed)
        } else {
          await closed
        }
        safeToDelete = true
      }
    } finally {
      if (safeToDelete) {
        rmSync(root, { recursive: true, force: true })
      }
    }
  }, 120_000)
})
