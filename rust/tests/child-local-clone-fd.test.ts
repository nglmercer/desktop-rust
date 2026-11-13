// @vitest-environment node
import assert from 'node:assert/strict'
import { execFile, spawn } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import { afterAll, beforeAll, describe, it } from 'vitest'
import { ALICE, BINARY, privateFile } from './fixtures/accounts'
import { Credentials } from './fixtures/credentials'

function assertNoDescriptorPaths(directory: string): void {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name)
    if (entry.isDirectory()) assertNoDescriptorPaths(file)
    else if (entry.isFile()) assert.doesNotMatch(readFileSync(file).toString('latin1'), /\/proc\/(?:self|[0-9]+)\/fd\//, `descriptor path persisted in ${file}`)
  }
}

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1' || process.platform !== 'linux')('child-local clone descriptor', () => {
  let suite: Credentials
  let brokerHelper: string
  beforeAll(async () => {
    suite = new Credentials()
    await suite.start()
    const source = suite.seed('child-fd-source', false)
    source.write('.gitattributes', '*.probe filter=cloneprobe\n')
    source.write('content.probe', 'filter-content\n')
    source.git('add', '.gitattributes', 'content.probe')
    source.git('commit', '-qm', 'add checkout filter fixture')
    suite.service.seed(source)
    brokerHelper = path.join(suite.root, 'child-fd-broker-helper')
    writeFileSync(brokerHelper, '#!/bin/sh\nif [ -n "${DESKTOP_TEST_CLONE_CHILD_FD:-}" ]; then\n  [ "$(stat -Lc "%d:%i" "/proc/self/fd/$DESKTOP_TEST_CLONE_CHILD_FD")" = "$DESKTOP_TEST_CLONE_CHILD_IDENTITY" ] || exit 77\n  [ "$(stat -Lc "%d:%i" "$HOME")" = "$DESKTOP_TEST_CLONE_HOME_IDENTITY" ] || exit 78\n  printf inherited > "$DESKTOP_TEST_HELPER_FD_MARKER"\nfi\nexec "$DESKTOP_TEST_REAL_HELPER" "$@"\n', { mode: 0o700 })
  }, 180_000)
  afterAll(async () => { if (suite) await suite.close() })

  it('survives parent exit and setsid through authenticated HTTPS and the credential helper', async () => {
    const stage = path.join(suite.root, 'child-fd-stage')
    const helper = path.join(suite.root, 'child-fd-helper')
    const marker = path.join(suite.root, 'child-fd-helper-ran')
    const result = path.join(suite.root, 'child-fd-result')
    const output = path.join(suite.root, 'child-fd-output')
    const release = path.join(suite.root, 'child-fd-release')
    mkdirSync(stage)
    const metadata = statSync(stage, { bigint: true })
    writeFileSync(helper, '#!/bin/sh\n[ "$(stat -Lc "%d:%i" /proc/self/fd/9)" = "$PROBE_IDENTITY" ] || exit 77\nprintf inherited > "$PROBE_MARKER"\nif [ "$1" = get ]; then printf "username=AliceCase\\npassword=%s\\n\\n" "$PROBE_TOKEN"; fi\n', { mode: 0o700 })
    const script = `exec 9<"$1"
setsid sh -c 'i=0; while [ ! -e "$6" ]; do i=$((i+1)); [ "$i" -lt 1500 ] || exit 1; sleep 0.02; done; "$1" -c credential.helper= -c "credential.helper=!$2" clone --no-checkout -- "$3" /proc/self/fd/9 >"$4" 2>&1; printf "%s" "$?" > "$5"' sh "$2" "$3" "$4" "$5" "$6" "$7" >/dev/null 2>&1 &`
    const url = `https://${suite.service.host}/fork/repo.git`
    suite.clearTransport()
    await promisify(execFile)('bash', ['-c', script, 'bash', stage, suite.realGit, helper, url, output, result, release], {
      env: { ...suite.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', GIT_TERMINAL_PROMPT: '0', PROBE_MARKER: marker, PROBE_TOKEN: ALICE, PROBE_IDENTITY: `${metadata.dev}:${metadata.ino}` },
      timeout: 10_000,
    })
    writeFileSync(release, 'continue after parent exit')
    const deadline = Date.now() + 30_000
    while (!existsSync(result) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
    assert.ok(existsSync(result), 'detached clone did not complete')
    assert.equal(readFileSync(result, 'utf8'), '0', 'detached authenticated clone failed')
    assert.equal(readFileSync(marker, 'utf8'), 'inherited')
    const show = await promisify(execFile)(suite.realGit, ['--git-dir', path.join(stage, '.git'), 'show', 'HEAD:file'], { env: suite.env })
    assert.equal(show.stdout, 'base\n')
    assertNoDescriptorPaths(path.join(stage, '.git'))
    suite.assertActor()
  }, 45_000)

  it('uses the child-local descriptor with the provisional account broker', async () => {
    const stage = path.join(suite.root, 'broker-child-fd-container', 'repository')
    const marker = path.join(suite.root, 'broker-child-fd-marker')
    mkdirSync(stage, { recursive: true })
    await promisify(execFile)('cargo', ['build', '-p', 'desktop-cli', '--locked'], { cwd: 'rust', timeout: 300_000 })
    suite.clearTransport()
    const fixture = await promisify(execFile)('cargo', [
      'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_authenticated_clone_fixture', '--', '--ignored',
    ], {
      cwd: 'rust', timeout: 300_000,
      env: {
        ...suite.env,
        RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
        CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
        DESKTOP_TEST_CLONE_STAGE: stage,
        DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
        DESKTOP_TEST_CLONE_HOST: suite.service.host,
        DESKTOP_TEST_CLONE_HELPER: brokerHelper,
        DESKTOP_TEST_REAL_HELPER: BINARY,
        DESKTOP_TEST_HELPER_FD_MARKER: marker,
      },
    })
    assert.match(fixture.stdout, /test .*child_fd_authenticated_clone_fixture .* ok/)
    assert.ok(existsSync(path.join(stage, '.git')))
    assert.equal(readFileSync(marker, 'utf8'), 'inherited')
    assertNoDescriptorPaths(path.join(stage, '.git'))
    suite.assertActor()
  }, 360_000)

  it('keeps the network child scoped to its own FD after its Rust parent is killed', async () => {
    const stage = path.join(suite.root, 'child-fd-crash-container', 'repository')
    const victim = path.join(suite.root, 'child-fd-crash-victim')
    const pidFile = path.join(suite.root, 'child-fd-crash-pid')
    const marker = path.join(suite.root, 'child-fd-crash-helper')
    mkdirSync(stage, { recursive: true })
    mkdirSync(victim)
    writeFileSync(path.join(victim, 'user-data'), 'preserve')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/git-upload-pack' })
    suite.clearTransport()
    const child = spawn('cargo', [
      'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_authenticated_clone_fixture', '--', '--ignored',
    ], {
      cwd: 'rust', stdio: 'ignore',
      env: {
        ...suite.env,
        RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
        CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
        DESKTOP_TEST_CLONE_STAGE: stage,
        DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
        DESKTOP_TEST_CLONE_HOST: suite.service.host,
        DESKTOP_TEST_CLONE_HELPER: brokerHelper,
        DESKTOP_TEST_REAL_HELPER: BINARY,
        DESKTOP_TEST_HELPER_FD_MARKER: marker,
        DESKTOP_TEST_CLONE_PID_FILE: pidFile,
      },
    })
    const exited = new Promise<void>((resolve, reject) => {
      child.once('close', () => resolve())
      child.once('error', reject)
    })
    try {
      const deadline = Date.now() + 60_000
      while ((!existsSync(pidFile) || !existsSync(suite.service.started)) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(existsSync(pidFile) && existsSync(suite.service.started), 'Rust clone did not reach the gated HTTPS transfer')
      assert.equal(readFileSync(marker, 'utf8'), 'inherited')
      const pid = Number(readFileSync(pidFile, 'utf8'))
      assert.ok(Number.isSafeInteger(pid) && pid > 1)
      const children = readdirSync(`/proc/${pid}/task`).flatMap(thread => {
        try { return readFileSync(`/proc/${pid}/task/${thread}/children`, 'utf8').trim().split(/\s+/).filter(Boolean).map(Number) } catch { return [] }
      })
      assert.ok(children.length > 0, 'expected a live Git child at the transfer gate')
      process.kill(pid, 'SIGKILL')
      await exited
      writeFileSync(suite.service.release, 'release')
      const stopDeadline = Date.now() + 15_000
      while (Date.now() < stopDeadline && children.some(childPid => {
        try { return readFileSync(`/proc/${childPid}/stat`, 'utf8').split(' ')[2] !== 'Z' } catch { return false }
      })) await new Promise(resolve => setTimeout(resolve, 25))
      assert.ok(children.every(childPid => {
        try { return readFileSync(`/proc/${childPid}/stat`, 'utf8').split(' ')[2] === 'Z' } catch { return true }
      }), 'Git child did not stop after its broker disappeared')
      await new Promise(resolve => setTimeout(resolve, 250))
      assert.equal(readFileSync(path.join(victim, 'user-data'), 'utf8'), 'preserve')
      assertNoDescriptorPaths(path.join(stage, '.git'))
      suite.assertActor()
    } finally {
      writeFileSync(suite.service.release, 'release')
      suite.service.update({ blockPath: undefined })
      if (child.exitCode === null) child.kill('SIGKILL')
      await exited
    }
  }, 90_000)

  it('checks out and records the selected account through the private clone flow', async () => {
    await promisify(execFile)('cargo', ['build', '-p', 'desktop-cli', '--locked'], { cwd: 'rust', timeout: 300_000 })
    const destination = path.join(suite.root, 'child-fd-complete')
    const registry = path.join(suite.root, 'child-fd-registry.json')
    const template = path.join(suite.root, 'child-fd-template')
    const hook = path.join(template, 'hooks', 'post-checkout')
    const hookResult = path.join(suite.root, 'child-fd-checkout-hook')
    const helperMarker = path.join(suite.root, 'child-fd-complete-helper')
    const filter = path.join(suite.root, 'child-fd-smudge')
    const filterResult = path.join(suite.root, 'child-fd-smudge-result')
    const expectedToken = path.join(suite.root, 'child-fd-expected-token')
    mkdirSync(path.dirname(hook), { recursive: true })
    privateFile(expectedToken, ALICE)
    writeFileSync(hook, '#!/bin/sh\nif env | grep -Eq "/proc/[0-9]+/fd/"; then printf unsafe > "$PROBE_HOOK_RESULT"; else printf safe > "$PROBE_HOOK_RESULT"; fi\n', { mode: 0o700 })
    writeFileSync(filter, '#!/bin/sh\nif env | grep -Eq "/proc/[0-9]+/fd/"; then exit 80; fi\ncredential=$(printf "protocol=https\\nhost=%s\\npath=fork/repo.git\\n\\n" "$PROBE_HOST" | git -c credential.useHttpPath=true credential fill) || exit 81\nexpected=$(cat "$PROBE_TOKEN_FILE") || exit 83\ncase "$credential" in *"password=$expected"*) printf safe > "$PROBE_FILTER_RESULT";; *) exit 82;; esac\ncat\n', { mode: 0o700 })
    await promisify(execFile)(suite.realGit, ['config', '--file', suite.globalConfig, 'init.templateDir', template], { env: suite.env })
    await promisify(execFile)(suite.realGit, ['config', '--file', suite.globalConfig, 'filter.cloneprobe.smudge', `'${filter.replaceAll("'", "'\\''")}'`], { env: suite.env })
    await promisify(execFile)(suite.realGit, ['config', '--file', suite.globalConfig, 'filter.cloneprobe.required', 'true'], { env: suite.env })
    suite.clearTransport()
    try {
      const fixture = await promisify(execFile)('cargo', [
      'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_selected_clone_fixture', '--', '--ignored',
      ], {
      cwd: 'rust', timeout: 300_000,
      env: {
        ...suite.env,
        RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
        CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
        DESKTOP_TEST_CLONE_DESTINATION: destination,
        DESKTOP_TEST_CLONE_REGISTRY: registry,
        DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
        DESKTOP_TEST_CLONE_HOST: suite.service.host,
        DESKTOP_TEST_CLONE_HELPER: brokerHelper,
        DESKTOP_TEST_REAL_HELPER: BINARY,
        DESKTOP_TEST_HELPER_FD_MARKER: helperMarker,
        PROBE_HOOK_RESULT: hookResult,
        PROBE_FILTER_RESULT: filterResult,
        PROBE_HOST: suite.service.host,
        PROBE_TOKEN_FILE: expectedToken,
      },
      })
      assert.match(fixture.stdout, /test .*child_fd_selected_clone_fixture .* ok/)
      assert.equal(readFileSync(helperMarker, 'utf8'), 'inherited')
      assert.equal(readFileSync(hookResult, 'utf8'), 'safe')
      assert.equal(readFileSync(filterResult, 'utf8'), 'safe')
      assert.equal(readFileSync(path.join(destination, 'content.probe'), 'utf8'), 'filter-content\n')
      assertNoDescriptorPaths(path.join(destination, '.git'))
      suite.assertActor()
    } finally {
      await promisify(execFile)(suite.realGit, ['config', '--file', suite.globalConfig, '--unset', 'init.templateDir'], { env: suite.env })
      await promisify(execFile)(suite.realGit, ['config', '--file', suite.globalConfig, '--remove-section', 'filter.cloneprobe'], { env: suite.env })
    }
  }, 360_000)

  it('preserves an obstruction and recovers the reviewed private clone', async () => {
    const destination = path.join(suite.root, 'child-fd-recovery')
    const registry = path.join(suite.root, 'child-fd-recovery-registry.json')
    const helperMarker = path.join(suite.root, 'child-fd-recovery-helper')
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    suite.clearTransport()
    try {
      const fixture = await promisify(execFile)('cargo', [
        'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_selected_clone_recovery_fixture', '--', '--ignored',
      ], {
        cwd: 'rust', timeout: 300_000,
        env: {
          ...suite.env,
          RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
          CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
          DESKTOP_TEST_CLONE_DESTINATION: destination,
          DESKTOP_TEST_CLONE_REGISTRY: registry,
          DESKTOP_TEST_CLONE_STARTED: suite.service.started,
          DESKTOP_TEST_CLONE_RELEASE: suite.service.release,
          DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
          DESKTOP_TEST_CLONE_HOST: suite.service.host,
          DESKTOP_TEST_CLONE_HELPER: brokerHelper,
          DESKTOP_TEST_REAL_HELPER: BINARY,
          DESKTOP_TEST_HELPER_FD_MARKER: helperMarker,
        },
      })
      assert.match(fixture.stdout, /test .*child_fd_selected_clone_recovery_fixture .* ok/)
      assert.equal(readFileSync(helperMarker, 'utf8'), 'inherited')
      assertNoDescriptorPaths(path.join(destination, '.git'))
      suite.assertActor()
    } finally {
      writeFileSync(suite.service.release, 'release')
      suite.service.update({ blockPath: undefined })
    }
  }, 360_000)

  it('refuses recovery after a different HOME makes the destination sensitive', async () => {
    const newHome = path.join(suite.root, 'changed-home')
    const destination = path.join(newHome, '.ssh', 'repo')
    const registry = path.join(suite.root, 'changed-home-registry.json')
    mkdirSync(path.dirname(destination), { recursive: true })
    rmSync(suite.service.started, { force: true })
    rmSync(suite.service.release, { force: true })
    suite.service.update({ blockPath: '/info/refs' })
    const env = {
      ...suite.env,
      RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
      CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
      DESKTOP_TEST_CLONE_DESTINATION: destination,
      DESKTOP_TEST_CLONE_REGISTRY: registry,
      DESKTOP_TEST_CLONE_STARTED: suite.service.started,
      DESKTOP_TEST_CLONE_RELEASE: suite.service.release,
      DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
      DESKTOP_TEST_CLONE_HOST: suite.service.host,
      DESKTOP_TEST_CLONE_HELPER: brokerHelper,
      DESKTOP_TEST_REAL_HELPER: BINARY,
      DESKTOP_TEST_HELPER_FD_MARKER: path.join(suite.root, 'changed-home-helper'),
    }
    try {
      const prepared = await promisify(execFile)('cargo', [
        'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_selected_clone_recovery_fixture', '--', '--ignored',
      ], { cwd: 'rust', timeout: 300_000, env: { ...env, DESKTOP_TEST_CLONE_PREPARE_RECOVERY_ONLY: '1' } })
      assert.match(prepared.stdout, /test .*child_fd_selected_clone_recovery_fixture .* ok/)
      const refused = await promisify(execFile)('cargo', [
        'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_changed_home_recovery_fixture', '--', '--ignored',
      ], { cwd: 'rust', timeout: 300_000, env: { ...env, HOME: newHome } })
      assert.match(refused.stdout, /test .*child_fd_changed_home_recovery_fixture .* ok/)
      assert.equal(existsSync(destination), false)
    } finally {
      writeFileSync(suite.service.release, 'release')
      suite.service.update({ blockPath: undefined })
    }
  }, 360_000)

  it('reports the retained staging path if journal admission fails', async () => {
    const parent = path.join(suite.root, 'admission-parent')
    const registry = path.join(suite.root, 'invalid-registry-directory')
    mkdirSync(parent)
    mkdirSync(registry)
    const fixture = await promisify(execFile)('cargo', [
      'test', '-p', 'desktop-core', '--locked', '--lib', 'child_fd_clone_admission_failure_fixture', '--', '--ignored',
    ], {
      cwd: 'rust', timeout: 300_000,
      env: {
        ...suite.env,
        RUSTUP_HOME: process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup'),
        CARGO_HOME: process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo'),
        DESKTOP_TEST_CLONE_DESTINATION: path.join(parent, 'repo'),
        DESKTOP_TEST_CLONE_REGISTRY: registry,
        DESKTOP_TEST_CLONE_URL: `https://${suite.service.host}/fork/repo.git`,
        DESKTOP_TEST_CLONE_HOST: suite.service.host,
        DESKTOP_TEST_CLONE_HELPER: brokerHelper,
        DESKTOP_TEST_REAL_HELPER: BINARY,
        DESKTOP_TEST_HELPER_FD_MARKER: path.join(suite.root, 'admission-helper-marker'),
      },
    })
    assert.match(fixture.stdout, /test .*child_fd_clone_admission_failure_fixture .* ok/)
  }, 360_000)
})
