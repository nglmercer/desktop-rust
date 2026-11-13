// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { build, mergeConfig } from 'vite'
import { describe, it } from 'vitest'
import rendererConfig from '../../vite.config'
import { Credentials } from './fixtures/credentials'

async function stopProcessGroup(pid: number, closed: Promise<void>) {
  const signal = (name: NodeJS.Signals) => {
    try { process.kill(-pid, name) } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
    }
  }
  const wait = async () => {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([closed.then(() => true), new Promise<false>(resolve => { timer = setTimeout(() => resolve(false), 5_000) })])
    } finally { if (timer !== undefined) clearTimeout(timer) }
  }
  signal('SIGTERM')
  if (!(await wait())) {
    signal('SIGKILL')
    if (!(await wait())) throw new Error('Wry process group did not close')
  }
}

// This opts into an actual WebKit shell and isolated HTTPS Git service.
describe.skipIf(process.env.DESKTOP_WRY_WEBVIEW_TEST !== '1' || process.env.DESKTOP_TEST_GIT_CREDS !== '1' || process.platform !== 'linux')('Wry selected-account clone', () => {
  it('retains and recovers a real HTTPS clone without exposing account data', async () => {
    const suite = new Credentials()
    let child: ReturnType<typeof spawn> | undefined
    let closed: Promise<void> | undefined
    let safeToDelete = true
    try {
      await suite.start()
      suite.seed('wry-source', false)
      const url = `https://${suite.service.host}/fork/repo.git`
      const destination = path.join(suite.root, 'wry-clone')
      const output = path.join(suite.root, 'bundle')
      const entry = path.join(suite.root, 'probe.ts')
      const binary = path.resolve('rust/target/debug/github-desktop-rs')
      const mountpoint = path.resolve('out/vite-dist')
      assert.ok(existsSync(binary), 'Build the debug desktop-app binary first')
      assert.ok(existsSync(mountpoint), 'Build the web renderer first')
      for (const directory of ['config', 'data', 'cache', 'runtime']) mkdirSync(path.join(suite.root, directory), { mode: 0o700 })
      const clone = path.resolve('app/src/ui/clone-repository/native-clone')
      const client = path.resolve('app/src/backend/client')
      writeFileSync(entry, `
import { nativeClone, listNativeCloneRecoveries, recoverNativeClone } from ${JSON.stringify(clone)}
import { backend } from ${JSON.stringify(client)}
;(window as any).runProbe = async () => {
  const destination = ${JSON.stringify(destination)}
  let progress = 0
  try {
    await nativeClone(${JSON.stringify(url)}, destination, ${JSON.stringify({host:suite.service.host,login:'AliceCase',id:101})}, undefined, () => progress++)
    throw new Error('clone unexpectedly replaced destination')
  } catch (error) {
    if (String(error).includes('unexpectedly replaced')) throw error
  }
  const raw = await backend().request('git_info', { operation: { type: 'list_clone_recoveries' } })
  if (!Array.isArray(raw) || raw.length !== 1) throw new Error('missing recovery')
  if (JSON.stringify(raw).includes(${JSON.stringify(url)}) || JSON.stringify(raw).includes('AliceCase') || 'account' in raw[0] || 'certificate' in raw[0]) throw new Error('recovery exposed account data')
  const item = (await listNativeCloneRecoveries())[0]
  if (!item?.recoverable || item.destination !== destination || !item.stagePath) throw new Error('invalid recovery')
  console.error('WRY_CLONE_READY')
  for (let i = 0; i < 1200 && await backend().pathExists(destination); i++) await new Promise(resolve => setTimeout(resolve, 25))
  const result = await recoverNativeClone(item.id, item.destination, item.stagePath)
  return { result, progress }
}
`)
      await build(mergeConfig(rendererConfig, { logLevel: 'silent', build: { outDir: output, emptyOutDir: true, rollupOptions: { input: entry }, lib: { entry, formats: ['iife'], name: 'WryCloneProbe', fileName: () => 'probe.js' } } }))
      writeFileSync(path.join(output, 'index.html'), `<!doctype html><meta charset="UTF-8"><script src="./probe.js"></script><script>window.runProbe().then(value => console.error('WRY_CLONE_SUCCESS ' + JSON.stringify(value)), error => console.error('WRY_CLONE_FAILURE ' + String(error && (error.stack || error))))</script>`)
      suite.service.update({ blockPath: '/info/refs' })
      child = spawn('unshare', ['--user', '--map-root-user', '--mount', '--propagation', 'private', 'sh', '-c', 'mount --bind "$1" "$2" || exit; exec xvfb-run -a "$3"', 'sh', output, mountpoint, binary], {
        detached: true,
        env: { ...suite.env, DESKTOP_USER_DATA: path.dirname(suite.registry), XDG_CONFIG_HOME: path.join(suite.root, 'config'), XDG_DATA_HOME: path.join(suite.root, 'data'), XDG_CACHE_HOME: path.join(suite.root, 'cache'), XDG_RUNTIME_DIR: path.join(suite.root, 'runtime'), WEBKIT_DISABLE_COMPOSITING_MODE: '1', NO_AT_BRIDGE: '1' },
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      safeToDelete = false
      closed = new Promise(resolve => child!.once('close', () => resolve()))
      let outputText = ''
      let failed: Error | undefined
      const collect = (chunk: Buffer) => { outputText += chunk.toString(); const match = outputText.match(/WRY_CLONE_FAILURE ([^\r\n]*)/); if (match) failed = new Error(match[1]) }
      child.stdout!.on('data', collect); child.stderr!.on('data', collect)
      child.once('error', error => { failed = error })
      child.once('exit', code => { if (!outputText.includes('WRY_CLONE_SUCCESS')) failed = new Error(`Wry exited ${code}: ${outputText}`) })
      const waitUntil = async (test: () => boolean) => {
        const deadline = Date.now() + 60_000
        while (!test() && !failed && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
        if (failed) throw failed
        assert.ok(test(), `Wry probe timed out: ${outputText}`)
      }
      await waitUntil(() => existsSync(suite.service.started))
      mkdirSync(destination); writeFileSync(path.join(destination, 'user-data'), 'preserve')
      writeFileSync(suite.service.release, 'release'); suite.service.update({ blockPath: undefined })
      await waitUntil(() => outputText.includes('WRY_CLONE_READY'))
      assert.equal(readFileSync(path.join(destination, 'user-data'), 'utf8'), 'preserve')
      rmSync(destination, { recursive: true })
      await waitUntil(() => outputText.includes('WRY_CLONE_SUCCESS'))
      const match = outputText.match(/WRY_CLONE_SUCCESS ([^\r\n]*)/)
      assert.ok(match)
      const result = JSON.parse(match[1])
      assert.equal(result.result.path, destination)
      assert.ok(result.progress > 0, 'expected progress over Wry')
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
      suite.assertActor()
    } finally {
      try {
        if (child?.pid && closed) await stopProcessGroup(child.pid, closed)
        else if (closed) await closed
        safeToDelete = true
      } finally {
        if (safeToDelete) await suite.close()
        else await suite.service.close()
      }
    }
  }, 180_000)
})
