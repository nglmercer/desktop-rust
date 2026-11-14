// @vitest-environment node
import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { existsSync } from 'node:fs'
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { build, mergeConfig } from 'vite'
import { describe, expect, it } from 'vitest'
import rendererConfig from '../../vite.config'

// A private mount namespace prevents this probe from replacing the shared renderer.
describe.skipIf(
  process.env.DESKTOP_WRY_WEBVIEW_TEST !== '1' || process.platform !== 'linux'
)('Wry pull', () => {
  it('runs hooks, rejects interactive hooks before mutation and reports conflicts', async () => {
    const root = await mkdtemp(path.join(tmpdir(), 'desktop-wry-pull-'))
    const env: NodeJS.ProcessEnv = Object.fromEntries(
      Object.entries(process.env).filter(
        ([key]) =>
          !key.startsWith('GIT_') &&
          key !== 'GH_TOKEN' &&
          key !== 'GITHUB_TOKEN'
      )
    )
    Object.assign(env, {
      HOME: root,
      USERPROFILE: root,
      XDG_CONFIG_HOME: path.join(root, 'config'),
      XDG_DATA_HOME: path.join(root, 'data'),
      XDG_CACHE_HOME: path.join(root, 'cache'),
      XDG_RUNTIME_DIR: path.join(root, 'runtime'),
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: path.join(root, 'gitconfig'),
      GIT_TEMPLATE_DIR: path.join(root, 'templates'),
      GIT_TERMINAL_PROMPT: '0',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      NO_AT_BRIDGE: '1',
    })
    const source = path.join(root, 'source')
    const target = path.join(root, 'target')
    const marker = path.join(root, 'hook-ran')
    const binary = path.resolve('rust/target/debug/github-desktop-rs')
    const mountpoint = path.resolve('out/vite-dist')
    let child: ChildProcess | undefined
    let closed: Promise<void> | undefined
    let safelyStopped = true
    try {
      expect(existsSync(binary), 'Build the debug desktop binary first').toBe(
        true
      )
      expect(existsSync(mountpoint), 'Build the web renderer first').toBe(true)
      for (const name of [
        'config',
        'data',
        'cache',
        'runtime',
        'templates',
        'source',
      ]) {
        await mkdir(path.join(root, name), { mode: 0o700 })
      }
      const git = (...args: string[]) =>
        execFileSync('git', args, {
          env,
          encoding: 'utf8',
          timeout: 20_000,
        }).trim()
      git('init', '-q', '-b', 'main', source)
      git('-C', source, 'config', 'user.name', 'Test')
      git('-C', source, 'config', 'user.email', 'test@example.invalid')
      git('-C', source, 'config', 'commit.gpgsign', 'false')
      await writeFile(path.join(source, 'base'), 'base\n')
      git('-C', source, 'add', 'base')
      git('-C', source, 'commit', '-qm', 'base')
      git('clone', '-q', '--no-local', source, target)
      git('-C', target, 'config', 'user.name', 'Test')
      git('-C', target, 'config', 'user.email', 'test@example.invalid')
      git('-C', target, 'config', 'commit.gpgsign', 'false')
      await writeFile(path.join(source, 'new'), 'new\n')
      git('-C', source, 'add', 'new')
      git('-C', source, 'commit', '-qm', 'new')
      const originalHead = git('-C', target, 'rev-parse', 'HEAD')
      const hook = path.join(target, '.git/hooks/post-merge')
      await mkdir(path.dirname(hook), { recursive: true })
      await writeFile(hook, `#!/bin/sh\nprintf ran > '${marker}'\n`)
      await chmod(hook, 0o755)
      const entry = path.join(root, 'probe.ts')
      const output = path.join(root, 'bundle')
      const client = path.resolve('app/src/backend/client')
      await writeFile(
        entry,
        `
import { backend, FetchPreviewUnsupported } from ${JSON.stringify(client)}
;(window as any).runProbe = async () => {
  const client = backend()
  await client.waitReady()
  const target = ${JSON.stringify(target)}
  const request = { remote: 'origin', expected_url: ${JSON.stringify(
    source
  )}, no_verify: false, report_progress: false, require_hook_callbacks: true }
  let rejected = false
  try { await client.pullRepositoryRemote(target, request) } catch (error) { rejected = error instanceof FetchPreviewUnsupported }
  if (!rejected) throw new Error('interactive hooks were not rejected before pull')
  console.error('WRY_PULL_REJECTED')
  for (let i = 0; i < 1200 && !(await client.pathExists(${JSON.stringify(
    path.join(root, 'continue')
  )})); i++) await new Promise(resolve => setTimeout(resolve, 25))
  await client.pullRepositoryRemote(target, { ...request, require_hook_callbacks: false })
  console.error('WRY_PULL_SUCCESS')
  for (let i = 0; i < 1200 && !(await client.pathExists(${JSON.stringify(
    path.join(root, 'conflict-ready')
  )})); i++) await new Promise(resolve => setTimeout(resolve, 25))
  const failure = await client.pullRepositoryRemote(target, { ...request, require_hook_callbacks: false })
  return failure
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
              name: 'WryPullProbe',
              fileName: () => 'probe.js',
            },
          },
        })
      )
      await writeFile(
        path.join(output, 'index.html'),
        `<!doctype html><meta charset="UTF-8"><script src="./probe.js"></script><script>window.runProbe().then(value => console.error('WRY_PULL_RESULT ' + JSON.stringify(value)), error => console.error('WRY_PULL_FAILURE ' + String(error && (error.stack || error))))</script>`
      )
      child = spawn(
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
        { env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] }
      )
      safelyStopped = false
      closed = new Promise(resolve => child!.once('close', () => resolve()))
      let logs = ''
      let failed: Error | undefined
      const collect = (chunk: Buffer) => {
        logs += chunk.toString()
        const match = logs.match(/WRY_PULL_FAILURE ([^\r\n]*)/)
        if (match) failed = new Error(match[1])
      }
      child.stdout!.on('data', collect)
      child.stderr!.on('data', collect)
      child.once('error', error => {
        failed = error
      })
      child.once('exit', code => {
        if (!logs.includes('WRY_PULL_RESULT'))
          failed = new Error(`Wry exited ${code}: ${logs}`)
      })
      const waitFor = async (value: string) => {
        const deadline = Date.now() + 60_000
        while (!logs.includes(value) && !failed && Date.now() < deadline)
          await delay(25)
        if (failed) throw failed
        expect(logs, `Wry probe timed out: ${logs.slice(-4000)}`).toContain(
          value
        )
      }
      await waitFor('WRY_PULL_REJECTED')
      expect(git('-C', target, 'rev-parse', 'HEAD')).toBe(originalHead)
      expect(existsSync(marker)).toBe(false)
      await writeFile(path.join(root, 'continue'), '')
      await waitFor('WRY_PULL_SUCCESS')
      expect(git('-C', target, 'rev-parse', 'HEAD')).toBe(
        git('-C', source, 'rev-parse', 'HEAD')
      )
      expect(await readFile(marker, 'utf8')).toBe('ran')
      await writeFile(path.join(source, 'base'), 'remote\n')
      git('-C', source, 'add', 'base')
      git('-C', source, 'commit', '-qm', 'remote')
      await writeFile(path.join(target, 'base'), 'local\n')
      git('-C', target, 'add', 'base')
      git('-C', target, 'commit', '-qm', 'local')
      git('-C', target, 'config', 'pull.rebase', 'false')
      await writeFile(path.join(root, 'conflict-ready'), '')
      await waitFor('WRY_PULL_RESULT')
      const match = logs.match(/WRY_PULL_RESULT ([^\r\n]*)/)
      expect(match).not.toBeNull()
      expect(JSON.parse(match![1]).errorKind).toBe('merge_conflicts')
      expect(git('-C', target, 'diff', '--name-only', '--diff-filter=U')).toBe(
        'base'
      )
    } finally {
      if (child?.pid && closed) {
        const signal = (name: NodeJS.Signals) => {
          try {
            process.kill(-child!.pid!, name)
          } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
          }
        }
        signal('SIGTERM')
        if (
          !(await Promise.race([
            closed.then(() => true),
            delay(5000).then(() => false),
          ]))
        ) {
          signal('SIGKILL')
          if (
            !(await Promise.race([
              closed.then(() => true),
              delay(5000).then(() => false),
            ]))
          )
            throw new Error('Wry process group did not close; fixture retained')
        }
        safelyStopped = true
      }
      if (safelyStopped) await rm(root, { recursive: true, force: true })
    }
  }, 180_000)
})
