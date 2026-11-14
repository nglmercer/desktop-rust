// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { afterAll, beforeAll, describe, it } from 'vitest'
import { ALICE, clean, object } from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

const prototype = path.resolve('rust/target/debug/examples/selected_clone_prototype')

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1' || process.platform !== 'linux')('selected-account clone prototype', () => {
  let suite: Credentials
  let url: string
  beforeAll(async () => {
    suite = new Credentials()
    await suite.start()
    suite.seed('clone-source', false)
    url = `https://${suite.service.host}/fork/repo.git`
    const build = spawnSync('cargo', ['build', '-p', 'desktop-cli', '--example', 'selected_clone_prototype', '--features', 'clone-prototype', '--locked'], { cwd: 'rust', encoding: 'utf8', timeout: 300_000 })
    assert.equal(build.status, 0, build.stderr)
  }, 360_000)
  afterAll(async () => { if (suite) await suite.close() })

  function run(destination: string, id = 101) {
    const result = spawnSync(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', String(id)], { env: suite.env, encoding: 'utf8', timeout: 60_000 })
    assert.ifError(result.error)
    clean(result.stdout)
    clean(result.stderr)
    return result
  }
  function intents() {
    const ledger = `${suite.registry}.clone-intents-v2.json`
    if (!existsSync(ledger)) return []
    return JSON.parse(readFileSync(ledger, 'utf8')) as Array<Record<string, unknown>>
  }

  it('downloads and checks out with the named actor and saves that account atomically', () => {
    const destination = path.join(suite.root, 'selected-clone')
    suite.clearTransport()
    const result = run(destination)
    assert.equal(result.status, 0, result.stderr)
    assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
    const config = readFileSync(path.join(destination, '.git', 'config'), 'utf8')
    clean(config)
    assert.ok(!config.includes('/proc/'))
    assert.ok(!config.includes('DESKTOP_RS_GIT'))
    suite.assertActor()
    const registry = object(JSON.parse(readFileSync(suite.registry, 'utf8')))
    const records = registry.records as Array<Record<string, unknown>>
    const record = records.find(row => row.path === destination)
    assert.deepEqual(object(object(record?.account).assignment), { mode: 'assigned', host: suite.service.host, login: 'AliceCase', id: 101 })
    assert.deepEqual(intents(), [])
    assert.ok(!result.stdout.includes(ALICE))
  }, 90_000)

  it('rejects a mismatched actor before creating a destination or recovery record', () => {
    const destination = path.join(suite.root, 'wrong-id')
    suite.clearTransport()
    const result = run(destination, 202)
    assert.notEqual(result.status, 0)
    assert.equal(existsSync(destination), false)
    assert.deepEqual(intents(), [])
    assert.ok(!suite.service.records().some(row => row.actor !== null))
  }, 90_000)

  it('refuses a signed-out account and user URL rewrites before transport', () => {
    const signedOut = path.join(suite.root, 'signed-out')
    suite.update({ signed_out: ['AliceCase'] })
    suite.clearTransport()
    try {
      assert.notEqual(run(signedOut).status, 0)
      assert.equal(existsSync(signedOut), false)
      assert.ok(!suite.service.records().some(row => row.actor !== null))
    } finally { suite.update({ signed_out: [] }) }
    const setting = `url.https://elsewhere.invalid/.insteadOf`
    const before = `https://${suite.service.host}/`
    const configured = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, setting, before], { encoding: 'utf8' })
    assert.equal(configured.status, 0, configured.stderr)
    try {
      const destination = path.join(suite.root, 'rewrite')
      suite.clearTransport()
      assert.notEqual(run(destination).status, 0)
      assert.equal(existsSync(destination), false)
      assert.ok(!suite.service.records().some(row => row.actor !== null))
    } finally {
      const reset = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, '--unset-all', setting], { encoding: 'utf8' })
      assert.equal(reset.status, 0, reset.stderr)
    }
  }, 90_000)

  it('installs user templates only after the credentialed transfer', () => {
    const template = path.join(suite.root, 'clone-template')
    const hooks = path.join(template, 'hooks')
    const marker = path.join(suite.root, 'checkout-hook-ran')
    mkdirSync(hooks, { recursive: true })
    writeFileSync(path.join(hooks, 'post-checkout'), `#!/bin/sh\nprintf ran > '${marker}'\n`, { mode: 0o700 })
    const set = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, 'init.templateDir', template], { encoding: 'utf8' })
    assert.equal(set.status, 0, set.stderr)
    try {
      const destination = path.join(suite.root, 'templated')
      const result = run(destination)
      assert.equal(result.status, 0, result.stderr)
      assert.equal(readFileSync(marker, 'utf8'), 'ran')
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
    } finally {
      const reset = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, '--unset-all', 'init.templateDir'], { encoding: 'utf8' })
      assert.equal(reset.status, 0, reset.stderr)
    }
  }, 90_000)

  it('does not overwrite a destination that appears during download and can recover publication', async () => {
    const destination = path.join(suite.root, 'contended')
    suite.service.update({ blockPath: '/info/refs' })
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''; let stderr = ''
    child.stdout.on('data', chunk => { stdout += chunk.toString() })
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
      mkdirSync(destination)
      writeFileSync(path.join(destination, 'user-data'), 'keep')
      writeFileSync(suite.service.release, 'release')
      const code = await new Promise<number | null>(resolve => child.once('close', resolve))
      assert.notEqual(code, 0)
      clean(stdout); clean(stderr)
      assert.equal(readFileSync(path.join(destination, 'user-data'), 'utf8'), 'keep')
      const pending = intents().find(item => item.destination === destination)
      assert.equal(pending?.phase, 'ready')
      rmSync(destination, { recursive: true })
      const recovered = spawnSync(prototype, ['recover', suite.registry, String(pending?.id)], { env: suite.env, encoding: 'utf8', timeout: 60_000 })
      assert.equal(recovered.status, 0, recovered.stderr)
      assert.equal(readFileSync(path.join(destination, 'file'), 'utf8'), 'base\n')
      assert.deepEqual(intents(), [])
    } finally {
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
    }
  }, 90_000)

  it('refuses recovery when the reviewed staging tree changes in place', async () => {
    const destination = path.join(suite.root, 'changed-stage')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
      mkdirSync(destination)
      writeFileSync(suite.service.release, 'release')
      const code = await new Promise<number | null>(resolve => child.once('close', resolve))
      assert.notEqual(code, 0)
      clean(stderr)
      const pending = intents().find(item => item.destination === destination)
      assert.equal(pending?.phase, 'ready')
      rmSync(destination, { recursive: true })
      writeFileSync(path.join(String(pending?.stage), 'file'), 'modified\n')
      const recovered = spawnSync(prototype, ['recover', suite.registry, String(pending?.id)], { env: suite.env, encoding: 'utf8', timeout: 60_000 })
      assert.notEqual(recovered.status, 0)
      clean(recovered.stderr)
      assert.equal(existsSync(destination), false)
      assert.equal(readFileSync(path.join(String(pending?.stage), 'file'), 'utf8'), 'modified\n')
      assert.equal(intents().find(item => item.destination === destination)?.phase, 'ready')
    } finally {
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
    }
  }, 90_000)

  it('retains an inspectable recovery record after cancellation during transfer', async () => {
    const destination = path.join(suite.root, 'cancelled')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
      child.kill('SIGINT')
      const code = await Promise.race([
        new Promise<number | null>(resolve => child.once('close', resolve)),
        new Promise<never>((_, reject) => setTimeout(() => reject(new Error('clone cancellation timed out')), 15_000)),
      ])
      assert.notEqual(code, 0)
      clean(stderr)
      assert.equal(existsSync(destination), false)
      assert.equal(intents().find(item => item.destination === destination)?.phase, 'downloading')
    } finally {
      writeFileSync(suite.service.release, 'release')
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
    }
  }, 90_000)

  it('waits for a cancelled checkout hook in selected-account staging', async () => {
    const destination = path.join(suite.root, 'cancelled-checkout')
    const template = path.join(suite.root, 'cancelled-checkout-template')
    const hooks = path.join(template, 'hooks')
    const started = path.join(suite.root, 'cancelled-checkout-started')
    const release = path.join(suite.root, 'cancelled-checkout-release')
    const late = path.join(suite.root, 'cancelled-checkout-late')
    mkdirSync(hooks, { recursive: true })
    writeFileSync(path.join(hooks, 'post-checkout'), '#!/bin/sh\ntrap "" INT\nprintf started > "$DESKTOP_CLONE_HOOK_STARTED"\nwhile [ ! -e "$DESKTOP_CLONE_HOOK_RELEASE" ]; do sleep 0.05; done\nprintf late > "$DESKTOP_CLONE_HOOK_LATE"\n', { mode: 0o700 })
    const set = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, 'init.templateDir', template], { encoding: 'utf8' })
    assert.equal(set.status, 0, set.stderr)
    suite.clearTransport()
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: { ...suite.env, DESKTOP_CLONE_HOOK_STARTED: started, DESKTOP_CLONE_HOOK_RELEASE: release, DESKTOP_CLONE_HOOK_LATE: late }, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    const closed = new Promise<number | null>((resolve, reject) => {
      child.once('error', reject)
      child.once('close', resolve)
    })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(started) && child.exitCode === null && Date.now() < deadline) {
        await new Promise(resolve => setTimeout(resolve, 25))
      }
      assert.ok(existsSync(started), `Git did not reach the staged checkout hook: ${stderr}`)
      const pending = intents().find(item => item.destination === destination)
      assert.equal(pending?.phase, 'checkout')
      assert.equal(existsSync(destination), false)
      assert.equal(typeof pending?.stage, 'string')
      const stage = String(pending?.stage)
      assert.equal(readFileSync(path.join(stage, 'file'), 'utf8'), 'base\n')
      assert.equal(child.kill('SIGINT'), true)
      let timer: ReturnType<typeof setTimeout> | undefined
      const code = await Promise.race([
        closed,
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('checkout cancellation timed out')), 15_000) }),
      ]).finally(() => clearTimeout(timer))
      assert.notEqual(code, 0)
      writeFileSync(release, 'release')
      await new Promise(resolve => setTimeout(resolve, 500))
      assert.equal(existsSync(late), false, 'checkout hook wrote after cancellation completed')
      assert.equal(existsSync(destination), false)
      assert.equal(readFileSync(path.join(stage, 'file'), 'utf8'), 'base\n')
      assert.equal(intents().find(item => item.destination === destination)?.phase, 'checkout')
      clean(stderr)
      suite.assertActor()
    } finally {
      writeFileSync(release, 'release')
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGKILL')
      }
      await closed.catch(() => {})
      const reset = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, '--unset-all', 'init.templateDir'], { encoding: 'utf8' })
      assert.equal(reset.status, 0, reset.stderr)
    }
  }, 90_000)

  it('does not publish when the selected account signs out during transfer', async () => {
    const destination = path.join(suite.root, 'account-changed')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
      suite.update({ signed_out: ['AliceCase'] })
      writeFileSync(suite.service.release, 'release')
      const code = await new Promise<number | null>(resolve => child.once('close', resolve))
      assert.notEqual(code, 0)
      clean(stderr)
      assert.equal(existsSync(destination), false)
      assert.equal(intents().find(item => item.destination === destination)?.phase, 'downloading')
    } finally {
      suite.update({ signed_out: [] })
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
    }
  }, 90_000)

  it('does not follow a replaced staging symlink into another directory', async () => {
    const destination = path.join(suite.root, 'stage-replaced')
    const victim = path.join(suite.root, 'stage-victim')
    mkdirSync(victim)
    writeFileSync(path.join(victim, 'user-data'), 'preserve')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(suite.service.started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(suite.service.started), 'Git transfer did not reach fixture')
      const pending = intents().find(item => item.destination === destination)
      assert.equal(pending?.phase, 'downloading')
      const stage = String(pending?.stage)
      renameSync(stage, `${stage}-moved`)
      symlinkSync(victim, stage, 'dir')
      writeFileSync(suite.service.release, 'release')
      const code = await new Promise<number | null>(resolve => child.once('close', resolve))
      assert.notEqual(code, 0)
      clean(stderr)
      assert.equal(existsSync(destination), false)
      assert.equal(readFileSync(path.join(victim, 'user-data'), 'utf8'), 'preserve')
      assert.equal(existsSync(path.join(victim, '.git')), false)
    } finally {
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
    }
  }, 90_000)

  it('keeps checkout pinned when the staging pathname is replaced', async () => {
    const destination = path.join(suite.root, 'checkout-stage-replaced')
    const victim = path.join(suite.root, 'checkout-stage-victim')
    const template = path.join(suite.root, 'blocking-template')
    const hooks = path.join(template, 'hooks')
    const started = path.join(suite.root, 'checkout-hook-started')
    const release = path.join(suite.root, 'checkout-hook-release')
    mkdirSync(victim)
    mkdirSync(hooks, { recursive: true })
    writeFileSync(path.join(victim, 'user-data'), 'preserve')
    writeFileSync(path.join(hooks, 'post-checkout'), `#!/bin/sh\nprintf started > '${started}'\nwhile [ ! -e '${release}' ]; do sleep 0.02; done\n`, { mode: 0o700 })
    const set = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, 'init.templateDir', template], { encoding: 'utf8' })
    assert.equal(set.status, 0, set.stderr)
    const child = spawn(prototype, [suite.registry, url, destination, suite.service.host, 'AliceCase', '101'], { env: suite.env, stdio: ['ignore', 'ignore', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', chunk => { stderr += chunk.toString() })
    try {
      const deadline = Date.now() + 30_000
      while (!existsSync(started) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(started), 'checkout hook did not start')
      const pending = intents().find(item => item.destination === destination)
      assert.equal(pending?.phase, 'checkout')
      const stage = String(pending?.stage)
      renameSync(stage, `${stage}-moved`)
      symlinkSync(victim, stage, 'dir')
      writeFileSync(release, 'release')
      const code = await new Promise<number | null>(resolve => child.once('close', resolve))
      assert.notEqual(code, 0)
      clean(stderr)
      assert.equal(existsSync(destination), false)
      assert.equal(readFileSync(path.join(victim, 'user-data'), 'utf8'), 'preserve')
      assert.equal(existsSync(path.join(victim, '.git')), false)
    } finally {
      writeFileSync(release, 'release')
      if (child.exitCode === null) child.kill('SIGKILL')
      const reset = spawnSync(suite.realGit, ['config', '--file', suite.globalConfig, '--unset-all', 'init.templateDir'], { encoding: 'utf8' })
      assert.equal(reset.status, 0, reset.stderr)
    }
  }, 90_000)
})
