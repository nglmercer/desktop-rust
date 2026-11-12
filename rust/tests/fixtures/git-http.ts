/** Shared local Git and LFS server for credential and checkout integration tests. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import path from 'node:path'
import { Worker } from 'node:worker_threads'
import {
  type AccountRepo,
  type Accounts,
  ALICE,
  BOB,
  ENV,
  type Json,
  object,
  privateFile,
  string,
} from './accounts'

interface Control {
  allowed: Record<string, string>
  rejectLfsBatch: boolean
  blockPath?: string
}
export class GitHttpService {
  readonly root: string
  readonly projects: string
  readonly objects: string
  readonly cert: string
  readonly controlFile: string
  readonly recordsFile: string
  readonly errorsFile: string
  readonly started: string
  readonly release: string
  host = ''
  control: Control = { allowed: {}, rejectLfsBatch: false }
  private worker?: Worker

  constructor(private readonly suite: Accounts) {
    this.root = path.join(suite.root, 'https')
    this.projects = path.join(this.root, 'projects')
    this.objects = path.join(this.root, 'objects')
    mkdirSync(this.projects, { recursive: true })
    mkdirSync(this.objects)
    this.cert = path.join(this.root, 'certificate.pem')
    const key = path.join(this.root, 'key.pem')
    const result = spawnSync(
      process.env.OPENSSL_BIN ?? 'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-days',
        '2',
        '-subj',
        '/CN=localhost',
        '-addext',
        'subjectAltName=IP:127.0.0.1,DNS:localhost',
        '-keyout',
        key,
        '-out',
        this.cert,
      ],
      { stdio: 'ignore', timeout: 20_000 }
    )
    assert.equal(result.status, 0, 'Could not create loopback TLS certificate')
    chmodSync(key, 0o600)
    chmodSync(this.cert, 0o600)
    this.controlFile = path.join(this.root, 'control.json')
    this.recordsFile = path.join(this.root, 'records.jsonl')
    this.errorsFile = path.join(this.root, 'errors.log')
    this.started = path.join(this.root, 'started')
    this.release = path.join(this.root, 'release')
    this.save()
  }
  async start(): Promise<void> {
    const execPath = spawnSync(this.suite.realGit, ['--exec-path'], {
      encoding: 'utf8',
    })
    assert.equal(execPath.status, 0, execPath.stderr)
    const backend = path.join(execPath.stdout.trim(), 'git-http-backend')
    assert.ok(existsSync(backend), 'Git smart HTTP backend is required')
    const worker = new Worker(
      new URL('./git-http-worker.ts', import.meta.url),
      {
        execArgv: ['--disable-warning=MODULE_TYPELESS_PACKAGE_JSON'],
        workerData: {
          cert: this.cert,
          key: path.join(this.root, 'key.pem'),
          backend,
          projects: this.projects,
          objects: this.objects,
          control: this.controlFile,
          records: this.recordsFile,
          errors: this.errorsFile,
          started: this.started,
          release: this.release,
          alice: ALICE,
          bob: BOB,
          environment: ENV,
        },
      }
    )
    this.worker = worker
    const value = await new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error('Git TLS fixture startup timed out')),
        12_000
      )
      worker.once('message', message => {
        clearTimeout(timer)
        resolve(message)
      })
      worker.once('error', error => {
        clearTimeout(timer)
        reject(error)
      })
      worker.once('exit', code => {
        if (code !== 0) {
          clearTimeout(timer)
          reject(new Error(`Git fixture exited: ${code}`))
        }
      })
    })
    this.host = string(object(value).host)
  }
  get url(): string {
    return `https://${this.host}`
  }
  save(): void {
    privateFile(this.controlFile, JSON.stringify(this.control))
  }
  update(value: Partial<Control>): void {
    this.control = { ...this.control, ...value }
    this.save()
  }
  seed(repo: AccountRepo): void {
    for (const file of [
      this.recordsFile,
      this.errorsFile,
      this.started,
      this.release,
    ])
      rmSync(file, { force: true })
    rmSync(this.objects, { recursive: true, force: true })
    mkdirSync(this.objects)
    const allowed: Record<string, string> = {}
    for (const name of [
      'fork/repo.git',
      'parent/repo.git',
      'other/repo.git',
      'fork/repo-other.git',
    ]) {
      const destination = path.join(this.projects, name)
      rmSync(destination, { recursive: true, force: true })
      mkdirSync(path.dirname(destination), { recursive: true })
      for (const args of [
        ['init', '--bare', '-q', destination],
        ['-C', destination, 'symbolic-ref', 'HEAD', 'refs/heads/main'],
        ['-C', destination, 'config', 'http.receivepack', 'true'],
      ]) {
        const result = spawnSync(this.suite.realGit, args, { encoding: 'utf8' })
        assert.equal(result.status, 0, result.stderr)
      }
      repo.git(
        '-c',
        'core.hooksPath=/dev/null',
        'push',
        '-q',
        destination,
        'HEAD:refs/heads/main'
      )
      allowed[`/${name}`] = name === 'fork/repo.git' ? 'alicecase' : 'bobcase'
    }
    this.control = { allowed, rejectLfsBatch: false }
    this.save()
  }
  records(): ReadonlyArray<Json> {
    return existsSync(this.recordsFile)
      ? readFileSync(this.recordsFile, 'utf8')
          .trim()
          .split('\n')
          .map(line => object(JSON.parse(line)))
      : []
  }
  errors(): string {
    return existsSync(this.errorsFile)
      ? readFileSync(this.errorsFile, 'utf8')
      : ''
  }
  async close(): Promise<void> {
    if (this.worker) await this.worker.terminate()
  }
}
