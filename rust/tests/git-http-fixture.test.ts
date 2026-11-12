// @vitest-environment node
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { request } from 'node:https'
import { afterAll, beforeAll, describe, it } from 'vitest'
import { Accounts, ALICE, BOB, clean } from './fixtures/accounts'
import { GitHttpService } from './fixtures/git-http'

describe('local Git and LFS fixture', () => {
  const suite = new Accounts()
  const service = new GitHttpService(suite)
  beforeAll(async () => {
    await service.start()
    suite.configure({ host: service.host })
    service.seed(suite.seed('fixture'))
  }, 30_000)
  afterAll(async () => {
    await service.close()
    rmSync(suite.root, { recursive: true, force: true })
  })

  function call(
    route: string,
    method: string,
    token: string,
    payload = Buffer.alloc(0)
  ): Promise<{ status: number; body: Buffer }> {
    return new Promise((resolve, reject) => {
      const req = request(
        `${service.url}${route}`,
        {
          method,
          ca: readFileSync(service.cert),
          headers: {
            Authorization: `Basic ${Buffer.from(`user:${token}`).toString('base64')}`,
            'Content-Length': payload.length,
          },
        },
        res => {
          const chunks: Buffer[] = []
          res.on('data', chunk => chunks.push(Buffer.from(chunk)))
          res.on('end', () =>
            resolve({
              status: res.statusCode ?? 0,
              body: Buffer.concat(chunks),
            })
          )
        }
      )
      req.on('error', reject)
      req.end(payload)
    })
  }
  it('serves a real Git repository and enforces per-repository identities', async () => {
    const output = await new Promise<string>((resolve, reject) => {
      const child = spawn(
        suite.realGit,
        [
          '-c',
          `http.sslCAInfo=${service.cert}`,
          '-c',
          `http.extraHeader=Authorization: Basic ${Buffer.from(`user:${ALICE}`).toString('base64')}`,
          'ls-remote',
          `${service.url}/fork/repo.git`,
        ],
        { env: { ...suite.env, GIT_TERMINAL_PROMPT: '0' } }
      )
      const chunks: Buffer[] = []
      child.stdout.on('data', chunk => chunks.push(Buffer.from(chunk)))
      child.once('error', reject)
      child.once('exit', code =>
        code === 0
          ? resolve(Buffer.concat(chunks).toString())
          : reject(new Error(`git exited ${code}`))
      )
    })
    assert.match(output, /refs\/heads\/main/)
    assert.equal(
      (
        await call(
          '/fork/repo.git/info/refs?service=git-upload-pack',
          'GET',
          BOB
        )
      ).status,
      401
    )
    clean(service.records())
    assert.equal(service.errors(), '')
  }, 30_000)
  it('uploads and downloads LFS objects and can reject batches', async () => {
    const body = Buffer.from('isolated LFS payload')
    const oid = createHash('sha256').update(body).digest('hex')
    const base = '/fork/repo.git/info/lfs'
    const batch = (operation: string) =>
      Buffer.from(
        JSON.stringify({ operation, objects: [{ oid, size: body.length }] })
      )
    const upload = JSON.parse(
      (
        await call(`${base}/objects/batch`, 'POST', ALICE, batch('upload'))
      ).body.toString()
    )
    assert.ok(upload.objects[0].actions.upload.href)
    assert.equal(
      (await call(`${base}/objects/${oid}`, 'PUT', ALICE, body)).status,
      200
    )
    const download = JSON.parse(
      (
        await call(`${base}/objects/batch`, 'POST', ALICE, batch('download'))
      ).body.toString()
    )
    assert.ok(download.objects[0].actions.download.href)
    assert.deepEqual(
      (await call(`${base}/objects/${oid}`, 'GET', ALICE)).body,
      body
    )
    service.update({ rejectLfsBatch: true })
    assert.equal(
      (await call(`${base}/objects/batch`, 'POST', ALICE, batch('download')))
        .status,
      422
    )
    clean(service.records())
  })
  it('can hold and release a request for cancellation tests', async () => {
    service.update({ blockPath: '/locks/verify' })
    const pending = call('/fork/repo.git/info/lfs/locks/verify', 'POST', ALICE)
    const deadline = Date.now() + 5_000
    while (!existsSync(service.started) && Date.now() < deadline)
      await new Promise(resolve => setTimeout(resolve, 10))
    assert.ok(existsSync(service.started))
    writeFileSync(service.release, '')
    assert.equal((await pending).status, 200)
  })
})
