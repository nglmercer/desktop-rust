/** Saved-account fixtures backed by real isolated Git HTTPS transfers. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  copyFileSync,
  existsSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import {
  Accounts,
  type AccountRepo,
  ALICE,
  BINARY,
  BOB,
  ENV,
  type Json,
  NAMED,
  clean,
  object,
  privateFile,
} from './accounts'
import { GitHttpService } from './git-http'

export class Credentials extends Accounts {
  readonly service = new GitHttpService(this)
  readonly nativeLog = path.join(this.root, 'native-helpers.jsonl')
  readonly gitLog = path.join(this.root, 'native-git.jsonl')
  readonly control = path.join(this.root, 'git-control.json')
  readonly gateStarted = path.join(this.root, 'git-gate-started')
  readonly gateRelease = path.join(this.root, 'git-gate-release')
  readonly globalConfig = path.join(this.root, 'global-config')
  helperCommand = ''

  async start(): Promise<void> {
    await this.service.start()
    privateFile(this.control, '{}')
    Object.assign(this.env, {
      CREDENTIALS_NATIVE_LOG: this.nativeLog,
      CREDENTIALS_GIT_LOG: this.gitLog,
      CREDENTIALS_CONTROL: this.control,
      CREDENTIALS_GATE_STARTED: this.gateStarted,
      CREDENTIALS_GATE_RELEASE: this.gateRelease,
      GIT_SSL_CAINFO: this.service.cert,
      SSL_CERT_FILE: this.service.cert,
      GIT_ASKPASS: 'false',
      SSH_ASKPASS: 'false',
    })
    const helper = path.join(this.root, 'bin', 'native-credential-helper')
    writeFileSync(
      helper,
      '#!/bin/sh\nexec "$ACCOUNT_NODE" "$CREDENTIALS_HELPER_FIXTURE" "$@"\n',
      { mode: 0o700 }
    )
    this.env.CREDENTIALS_HELPER_FIXTURE = path.resolve(
      'rust/tests/fixtures/native-credential-helper.ts'
    )
    this.helperCommand = `!'${helper.replaceAll("'", "'\\''")}'`
    this.env.CREDENTIALS_HOSTILE_HELPER = this.helperCommand
    for (const args of [
      ['credential.helper', this.helperCommand],
      ['http.sslCAInfo', this.service.cert],
    ]) {
      const result = spawnSync(
        this.realGit,
        ['config', '--file', this.globalConfig, ...args],
        { encoding: 'utf8' }
      )
      assert.equal(result.status, 0, result.stderr)
    }
    const build = spawnSync(
      'cargo',
      [
        'build',
        '-p',
        'desktop-cli',
        '--example',
        'git_credentials_wrapper',
        '--locked',
      ],
      { cwd: 'rust', encoding: 'utf8', timeout: 120_000 }
    )
    assert.equal(build.status, 0, build.stderr)
    const config = path.join(this.root, 'git-wrapper-config.json')
    privateFile(
      config,
      JSON.stringify({
        real_git: this.realGit,
        alice: ALICE,
        bob: BOB,
        environment: ENV,
      })
    )
    this.env.CREDENTIALS_WRAPPER_CONFIG = config
    const wrapper = path.join(this.root, 'bin', 'git')
    copyFileSync(
      path.join(path.dirname(BINARY), 'examples', 'git_credentials_wrapper'),
      wrapper
    )
    chmodSync(wrapper, 0o700)
    this.configure({ host: this.service.host })
  }
  assignedTarget(target: Json): Json {
    return target.mode === 'assigned'
      ? { ...target, host: this.service.host }
      : target
  }
  override preview(repo: AccountRepo, target: Json = NAMED): Json {
    return super.preview(repo, this.assignedTarget(target))
  }
  override assign(repo: AccountRepo, target: Json = NAMED): Json {
    return super.assign(repo, this.assignedTarget(target))
  }
  override seed(name: string, save = true): AccountRepo {
    const repo = super.seed(name, save)
    this.service.seed(repo)
    this.clearTransport()
    return repo
  }
  records(file: string): ReadonlyArray<Json> {
    return existsSync(file)
      ? readFileSync(file, 'utf8')
          .trim()
          .split('\n')
          .map(line => object(JSON.parse(line)))
      : []
  }
  clearTransport(): void {
    for (const file of [this.service.recordsFile, this.nativeLog, this.gitLog])
      rmSync(file, { force: true })
  }
  assertActor(actor = 'alicecase'): void {
    const rows = this.service.records().filter(row => row.actor !== null)
    assert.ok(rows.length, 'Git never authenticated to the local service')
    assert.ok(
      rows.every(row => row.actor === actor),
      JSON.stringify(rows)
    )
    assert.ok(
      !existsSync(this.nativeLog),
      JSON.stringify(this.records(this.nativeLog))
    )
    assert.ok(
      this.records(this.gitLog).every(
        row => row.argv_clean && row.alice_environment_clean
      )
    )
    assert.equal(this.service.errors(), '')
  }
  assertPrivateStorage(repo: AccountRepo): void {
    for (const file of [
      this.registry,
      this.globalConfig,
      path.join(repo.root, '.git', 'config'),
    ])
      if (existsSync(file)) clean(readFileSync(file, 'utf8'))
    clean(this.records(this.nativeLog))
    clean(this.records(this.gitLog))
    clean(this.service.records())
  }
  async close(): Promise<void> {
    await this.service.close()
    this.dispose()
  }
}
