// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'vitest'

function startFixture(hang = false, acknowledge = true, argument?: string) {
  const root = mkdtempSync(path.join(tmpdir(), 'desktop-prod-supervisor-'))
  const app = JSON.parse(readFileSync('app/package.json', 'utf8')) as {
    productName: string
    bundleID: string
    version: string
  }
  const platform = process.platform === 'darwin' ? 'macos' : 'linux'
  const packageDir = path.join(root, `${app.productName}-${platform}-${process.arch}`)
  mkdirSync(path.join(packageDir, 'bin'), { recursive: true })
  mkdirSync(path.join(packageDir, 'meta'), { recursive: true })
  writeFileSync(
    path.join(packageDir, 'meta/package.json'),
    JSON.stringify({
      productName: app.productName,
      bundleID: app.bundleID,
      version: app.version,
      channel: 'production',
      profile: 'release',
      platform,
      arch: process.arch,
      executable: 'bin/desktop-rs',
    })
  )
  writeFileSync(
    path.join(packageDir, 'bin/github-desktop-rs'),
    `#!/usr/bin/env node
const fs = require('node:fs')
const quit = process.env.DESKTOP_PACKAGE_QUIT_FILE
if (process.argv.includes('--version')) {
  fs.writeFileSync(require('node:path').join(require('node:path').dirname(quit), 'result'), 'drained', { flag: 'wx' })
  process.stdout.write('fixture version\\n')
  process.exit(0)
}
process.stdout.write('READY ' + process.pid + ' ' + quit + '\\n')
let requested = false
setInterval(() => {
  if (!requested && fs.existsSync(quit)) {
    requested = true
    process.stdout.write('DRAINING\\n')
    if (!process.env.TEST_PACKAGE_HANG) setTimeout(() => {
      process.stderr.write('desktop-package-shutdown:drained\\n')
      if (process.env.TEST_PACKAGE_ACK === '1') fs.writeFileSync(require('node:path').join(require('node:path').dirname(quit), 'result'), 'drained', { flag: 'wx' })
      process.exit(0)
    }, 800)
  }
}, 20)
`,
    { mode: 0o755 }
  )
  const child = spawn(
    process.execPath,
    ['--import', 'tsx', 'script/start-rust-package.ts', ...(argument ? [argument] : [])],
    {
      cwd: process.cwd(),
      detached: true,
      env: {
        ...process.env,
        RELEASE_CHANNEL: 'production',
        DESKTOP_RUST_DIST_DIR: root,
        TEST_PACKAGE_HANG: hang ? '1' : '',
        TEST_PACKAGE_ACK: acknowledge ? '1' : '',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    }
  )
  let output = ''
  child.stdout.on('data', chunk => { output += String(chunk) })
  child.stderr.on('data', chunk => { output += String(chunk) })
  const closed = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
    (resolve, reject) => {
      child.once('error', reject)
      child.once('close', (code, signal) => resolve({ code, signal }))
    }
  )
  return { child, root, closed, output: () => output }
}

async function waitFor(output: () => string, expression: RegExp, timeout: number) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const match = expression.exec(output())
    if (match) return match
    await new Promise(resolve => setTimeout(resolve, 20))
  }
  throw new Error(`Timed out waiting for fixture: ${output()}`)
}

async function bounded<T>(value: Promise<T>, timeout: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      value,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Supervisor did not exit')), timeout)
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function launchAgain(root: string) {
  const child = spawn(process.execPath, ['--import', 'tsx', 'script/start-rust-package.ts'], {
    cwd: process.cwd(),
    detached: true,
    env: { ...process.env, RELEASE_CHANNEL: 'production', DESKTOP_RUST_DIST_DIR: root },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let output = ''
  child.stdout.on('data', chunk => { output += String(chunk) })
  child.stderr.on('data', chunk => { output += String(chunk) })
  const closed = new Promise<number | null>((resolve, reject) => {
    child.once('error', reject)
    child.once('close', code => resolve(code))
  })
  try {
    return { code: await bounded(closed, 5_000), output }
  } finally {
    const hostPid = Number(/READY (\d+)/.exec(output)?.[1])
    if (hostPid && isAlive(hostPid)) {
      try { process.kill(-hostPid, 'SIGKILL') } catch {}
    }
    if (child.pid && child.exitCode === null && child.signalCode === null) {
      try { process.kill(-child.pid, 'SIGKILL') } catch {}
    }
  }
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

describe.skipIf(!['linux', 'darwin'].includes(process.platform) || !['x64', 'arm64'].includes(process.arch))('packaged desktop supervisor', () => {
  it('releases its lock after an acknowledged early version exit', async () => {
    const run = startFixture(false, true, '--version')
    try {
      const result = await bounded(run.closed, 10_000)
      assert.equal(result.code, 0, run.output())
      assert.match(run.output(), /fixture version/)
      assert.equal(readdirSync(run.root).some(name => name.endsWith('.lock')), false)
    } finally {
      if (run.child.pid && run.child.exitCode === null && run.child.signalCode === null) {
        try { process.kill(-run.child.pid, 'SIGKILL') } catch {}
      }
      await bounded(run.closed, 5_000).catch(() => {})
      rmSync(run.root, { recursive: true, force: true })
    }
  }, 20_000)

  it('drains the detached host when only the supervisor receives repeated signals', async () => {
    const run = startFixture()
    let hostPid: number | undefined
    try {
      const ready = await waitFor(run.output, /READY (\d+) ([^\n]+)/, 10_000)
      hostPid = Number(ready[1])
      const control = path.dirname(ready[2])
      assert.ok(isAlive(hostPid))
      const competing = await launchAgain(run.root)
      assert.equal(competing.code, 1, competing.output)
      assert.match(competing.output, /lock/i)
      assert.equal(run.child.kill('SIGTERM'), true)
      await waitFor(run.output, /DRAINING/, 5_000)
      for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP'] as const) {
        assert.equal(run.child.kill(signal), true)
        await new Promise(resolve => setTimeout(resolve, 50))
      }
      const result = await bounded(run.closed, 6_000)
      assert.equal(result.code, 0, run.output())
      assert.match(run.output(), /desktop-package-shutdown:drained/)
      assert.equal(isAlive(hostPid), false)
      assert.equal(existsSync(control), false)
      assert.equal(readdirSync(run.root).some(name => name.endsWith('.lock')), false)
    } finally {
      if (hostPid && isAlive(hostPid)) {
        try { process.kill(-hostPid, 'SIGKILL') } catch {}
      }
      if (run.child.pid && run.child.exitCode === null && run.child.signalCode === null) {
        try { process.kill(-run.child.pid, 'SIGKILL') } catch {}
      }
      await bounded(run.closed, 5_000).catch(() => {})
      rmSync(run.root, { recursive: true, force: true })
    }
  }, 25_000)

  it('reports failure when the host exits without confirming a drain', async () => {
    const run = startFixture(false, false)
    let hostPid: number | undefined
    try {
      const ready = await waitFor(run.output, /READY (\d+) ([^\n]+)/, 10_000)
      hostPid = Number(ready[1])
      assert.equal(run.child.kill('SIGTERM'), true)
      const result = await bounded(run.closed, 6_000)
      assert.equal(result.code, 1, run.output())
      assert.match(run.output(), /shutdown was not confirmed as drained/)
      assert.equal(isAlive(hostPid), false)
      assert.equal(readdirSync(run.root).some(name => name.endsWith('.lock')), true)
      const retry = await launchAgain(run.root)
      assert.equal(retry.code, 1, retry.output)
      assert.match(retry.output, /lock/i)
    } finally {
      if (hostPid && isAlive(hostPid)) {
        try { process.kill(-hostPid, 'SIGKILL') } catch {}
      }
      if (run.child.pid && run.child.exitCode === null && run.child.signalCode === null) {
        try { process.kill(-run.child.pid, 'SIGKILL') } catch {}
      }
      await bounded(run.closed, 5_000).catch(() => {})
      rmSync(run.root, { recursive: true, force: true })
    }
  }, 25_000)

  it('forces a nonresponsive host to exit and reports failure', async () => {
    const run = startFixture(true)
    let hostPid: number | undefined
    try {
      const ready = await waitFor(run.output, /READY (\d+) ([^\n]+)/, 10_000)
      hostPid = Number(ready[1])
      const control = path.dirname(ready[2])
      assert.equal(run.child.kill('SIGTERM'), true)
      await waitFor(run.output, /DRAINING/, 5_000)
      const result = await bounded(run.closed, 12_000)
      assert.equal(result.code, 1, run.output())
      assert.match(run.output(), /did not drain; forcing termination/)
      assert.equal(isAlive(hostPid), false)
      assert.equal(existsSync(control), false)
    } finally {
      if (hostPid && isAlive(hostPid)) {
        try { process.kill(-hostPid, 'SIGKILL') } catch {}
      }
      if (run.child.pid && run.child.exitCode === null && run.child.signalCode === null) {
        try { process.kill(-run.child.pid, 'SIGKILL') } catch {}
      }
      await bounded(run.closed, 5_000).catch(() => {})
      rmSync(run.root, { recursive: true, force: true })
    }
  }, 25_000)
})
