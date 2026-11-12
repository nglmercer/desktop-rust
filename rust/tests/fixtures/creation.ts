/** Local TLS and saved-account fixtures for pull-request creation. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { Worker } from 'node:worker_threads'
import {
  Accounts,
  AccountRepo,
  ALICE,
  BOB,
  ENV,
  type Json,
  object,
  privateFile,
  string,
} from './accounts'

export const PR_URL = 'https://github.com/fork/repo/pull/7'
export const ACTION = {
  type: 'create_pull_request',
  title: 'Literal {owner}/{repo}/{branch} $title 雪',
  body: 'First line\n\nLiteral {owner}/{repo}/{branch}, $(no-shell), `quotes`, \\ and 雪\n',
  base: 'base-{branch}',
  head: 'fork:head-{owner}',
  draft: true,
}
interface IControl {
  readonly failures: Record<string, string>
  readonly url: string | null
  readonly node: Record<string, unknown> | null
  readonly pages: ReadonlyArray<ReadonlyArray<unknown>>
  readonly page_override?: unknown
  readonly gate?: string
}
function records(file: string): ReadonlyArray<Json> {
  return existsSync(file)
    ? readFileSync(file, 'utf8')
        .trim()
        .split('\n')
        .map(line => object(JSON.parse(line)))
    : []
}
function commandPath(name: string): string {
  const result = spawnSync('sh', ['-c', 'command -v "$1"', 'fixture', name], {
    encoding: 'utf8',
  })
  assert.equal(result.status, 0, `Missing ${name}`)
  return result.stdout.trim()
}
export class CreationService {
  readonly cert: string
  readonly controlFile: string
  readonly recordsFile: string
  readonly errorsFile: string
  readonly started: string
  readonly release: string
  endpoint = ''
  proxy = ''
  control: IControl = {
    failures: {},
    url: PR_URL,
    node: {
      id: 'R_fixture',
      nameWithOwner: 'fork/repo',
      url: 'https://github.com/fork/repo',
    },
    pages: [[]],
  }
  private worker: Worker | undefined

  constructor(private readonly suite: Accounts) {
    const root = suite.root
    this.cert = path.join(root, 'api-cert.pem')
    const key = path.join(root, 'api-key.pem')
    const openssl = process.env.OPENSSL_BIN ?? commandPath('openssl')
    const generated = spawnSync(
      openssl,
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-days',
        '2',
        '-subj',
        '/CN=api.github.com',
        '-addext',
        'subjectAltName=DNS:api.github.com,DNS:github.com,DNS:localhost,IP:127.0.0.1',
        '-keyout',
        key,
        '-out',
        this.cert,
      ],
      { stdio: 'ignore', timeout: 20_000 }
    )
    assert.equal(generated.status, 0, 'Could not create local TLS certificate')
    chmodSync(key, 0o600)
    chmodSync(this.cert, 0o600)
    this.controlFile = path.join(root, 'api-control.json')
    this.recordsFile = path.join(root, 'api-records.jsonl')
    this.errorsFile = path.join(root, 'api-errors.log')
    this.started = path.join(root, 'api-started')
    this.release = path.join(root, 'api-release')
    this.save()
  }
  async start(): Promise<void> {
    const worker = new Worker(
      new URL('./creation-api-worker.ts', import.meta.url),
      {
        execArgv: ['--disable-warning=MODULE_TYPELESS_PACKAGE_JSON'],
        workerData: {
          cert: this.cert,
          key: path.join(this.suite.root, 'api-key.pem'),
          scenario: this.suite.scenario,
          control: this.controlFile,
          records: this.recordsFile,
          errors: this.errorsFile,
          started: this.started,
          release: this.release,
        },
      }
    )
    this.worker = worker
    const ready = await new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error('TLS fixture startup timed out')),
        12_000
      )
      worker.once('message', value => {
        clearTimeout(timer)
        resolve(value)
      })
      worker.once('error', error => {
        clearTimeout(timer)
        reject(error)
      })
      worker.once('exit', code => {
        if (code !== 0) {
          clearTimeout(timer)
          reject(new Error('TLS fixture exited early'))
        }
      })
    })
    const data = object(ready)
    this.endpoint = string(data.endpoint)
    this.proxy = string(data.proxy)
  }
  save(): void {
    privateFile(this.controlFile, JSON.stringify(this.control))
  }
  update(value: Partial<IControl>): void {
    this.control = { ...this.control, ...value }
    this.save()
  }
  reset(): void {
    for (const file of [
      this.recordsFile,
      this.errorsFile,
      this.started,
      this.release,
    ])
      rmSync(file, { force: true })
    this.control = {
      failures: {},
      url: PR_URL,
      node: {
        id: 'R_fixture',
        nameWithOwner: 'fork/repo',
        url: 'https://github.com/fork/repo',
      },
      pages: [[]],
    }
    this.save()
  }
  records(): ReadonlyArray<Json> {
    return records(this.recordsFile)
  }
  mutations(): ReadonlyArray<Json> {
    return this.records().filter(row => row.stage === 'mutation')
  }
  errors(): string {
    return existsSync(this.errorsFile)
      ? readFileSync(this.errorsFile, 'utf8')
      : ''
  }
  async close(): Promise<void> {
    if (this.worker !== undefined) {
      this.worker.postMessage('close')
      await this.worker.terminate()
      this.worker = undefined
    }
  }
}

export class Creation extends Accounts {
  service: CreationService | undefined
  readonly gitLog = path.join(this.root, 'git-calls.jsonl')

  async start(): Promise<CreationService> {
    const service = new CreationService(this)
    this.service = service
    try {
      await service.start()
    } catch (error) {
      await service.close()
      throw error
    }
    const bin = path.join(this.root, 'bin')
    const gh = path.join(bin, 'gh')
    writeFileSync(
      gh,
      '#!/bin/sh\nexec "$ACCOUNT_NODE" --disable-warning=MODULE_TYPELESS_PACKAGE_JSON "$CREATE_GH_FIXTURE" "$@"\n',
      { mode: 0o700 }
    )
    const guard = path.join(bin, 'git-local-only')
    copyFileSync(path.join(bin, 'git'), guard)
    chmodSync(guard, 0o700)
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
      { cwd: path.resolve('.'), encoding: 'utf8', timeout: 120_000 }
    )
    assert.equal(build.status, 0, build.stderr)
    copyFileSync(
      path.resolve('rust/target/debug/examples/git_credentials_wrapper'),
      path.join(bin, 'git')
    )
    chmodSync(path.join(bin, 'git'), 0o700)
    const wrapper = path.join(this.root, 'git-wrapper.json')
    privateFile(
      wrapper,
      JSON.stringify({
        real_git: guard,
        alice: ALICE,
        bob: BOB,
        environment: ENV,
      })
    )
    const control = path.join(this.root, 'git-control.json')
    privateFile(control, '{}')
    const globalConfig = path.join(this.root, 'global.config')
    privateFile(globalConfig, '')
    const templates = path.join(this.root, 'empty-templates')
    mkdirSync(templates)
    Object.assign(this.env, {
      CREATE_GH_FIXTURE: path.resolve('rust/tests/fixtures/creation-gh.ts'),
      CREATE_API_ENDPOINT: service.endpoint,
      CREATE_API_CA: service.cert,
      CREATE_API_PROXY: service.proxy,
      CREATE_REAL_GH: process.env.GH_BIN ?? commandPath('gh'),
      GH_CONFIG_DIR: path.join(this.root, 'gh-config'),
      GH_NO_UPDATE_NOTIFIER: '1',
      GH_NO_EXTENSION_UPDATE_NOTIFIER: '1',
      GIT_CONFIG_GLOBAL: globalConfig,
      GIT_CONFIG_SYSTEM: globalConfig,
      GIT_ATTR_NOSYSTEM: '1',
      GIT_TEMPLATE_DIR: templates,
      CREDENTIALS_WRAPPER_CONFIG: wrapper,
      CREDENTIALS_GIT_LOG: this.gitLog,
      CREDENTIALS_CONTROL: control,
      CREDENTIALS_GATE_STARTED: path.join(this.root, 'git-started'),
      CREDENTIALS_GATE_RELEASE: path.join(this.root, 'git-release'),
    })
    for (const name of Object.keys(this.env))
      if (
        name.startsWith('GIT_TRACE') ||
        name.startsWith('GIT_CONFIG_KEY_') ||
        name.startsWith('GIT_CONFIG_VALUE_') ||
        [
          'GIT_CURL_VERBOSE',
          'GIT_TRANSFER_TRACE',
          'GIT_LOG_STATS',
          'LFS_DEBUG_HTTP',
          'GIT_LFS_PROGRESS',
          'GIT_CONFIG',
          'GIT_COMMON_DIR',
          'GIT_OBJECT_DIRECTORY',
          'GIT_ALTERNATE_OBJECT_DIRECTORIES',
        ].includes(name)
      )
        delete this.env[name]
    return service
  }
  seedUnborn(name: string): AccountRepo {
    assert.ok(this.service)
    this.configure()
    this.service.reset()
    const root = path.join(this.root, name)
    const init = spawnSync(this.realGit, ['init', '-q', '-b', 'main', root], {
      env: this.env,
      encoding: 'utf8',
    })
    assert.equal(init.status, 0, init.stderr)
    const repo = new AccountRepo(root, this)
    repo.write('file', 'unchanged working file\n')
    repo.git('remote', 'add', 'origin', 'https://github.com/fork/repo.git')
    this.cli(['repositories', 'add', root])
    this.assign(repo)
    return repo
  }
  clear(): void {
    super.clear()
    rmSync(this.gitLog, { force: true })
    this.service?.reset()
  }
  gitCalls(): ReadonlyArray<Json> {
    return records(this.gitLog)
  }
  evidence(repo: AccountRepo): unknown {
    const rows: Array<[string, string]> = []
    const visit = (dir: string, relative: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name)
        const name = path.join(relative, entry.name)
        if (entry.isDirectory()) visit(full, name)
        else if (entry.isFile())
          rows.push([
            name,
            createHash('sha256').update(readFileSync(full)).digest('hex'),
          ])
      }
    }
    visit(repo.root, '')
    return rows.sort(([a], [b]) => a.localeCompare(b))
  }
  create(
    repo: AccountRepo,
    action: Json = ACTION,
    basis?: unknown,
    fail = false
  ): Json {
    return this.rpc(
      repo,
      'github',
      {
        request: {
          action,
          ...(basis === undefined ? {} : { account_basis: basis }),
        },
      },
      { fail }
    )
  }
  async close(): Promise<void> {
    await this.service?.close()
    this.dispose()
  }
}
