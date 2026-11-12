/** Authenticated, loopback-only Git smart HTTP and Git LFS fixture. */
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  appendFileSync,
  existsSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'
import { createServer } from 'node:https'
import path from 'node:path'
import { parentPort, workerData } from 'node:worker_threads'

interface Settings {
  cert: string
  key: string
  backend: string
  projects: string
  objects: string
  control: string
  records: string
  errors: string
  started: string
  release: string
  alice: string
  bob: string
  environment: string
}
interface Control {
  allowed: Record<string, string>
  rejectLfsBatch: boolean
  blockPath?: string
}
const settings: Settings = workerData
if (parentPort === null) throw new Error('Git fixture requires a parent')
const parent = parentPort
const server = createServer(
  { cert: readFileSync(settings.cert), key: readFileSync(settings.key) },
  async (req, res) => {
    try {
      const control: Control = JSON.parse(
        readFileSync(settings.control, 'utf8')
      )
      const url = new URL(req.url ?? '/', 'https://localhost')
      const chunks: Buffer[] = []
      for await (const chunk of req) chunks.push(Buffer.from(chunk))
      const body = Buffer.concat(chunks)
      const authorization = req.headers.authorization ?? ''
      let credential = ''
      if (authorization.startsWith('Basic ')) {
        const decoded = Buffer.from(authorization.slice(6), 'base64').toString()
        credential = decoded.slice(decoded.indexOf(':') + 1)
      } else if (authorization.startsWith('Bearer '))
        credential = authorization.slice(7)
      const actor =
        credential === settings.alice
          ? 'alicecase'
          : credential === settings.bob
            ? 'bobcase'
            : credential === settings.environment
              ? 'environment-user'
              : authorization
                ? 'invalid'
                : null
      const repository = Object.keys(control.allowed).find(
        name => url.pathname === name || url.pathname.startsWith(`${name}/`)
      )
      const rejected =
        control.rejectLfsBatch &&
        repository !== undefined &&
        actor === control.allowed[repository] &&
        req.method === 'POST' &&
        url.pathname.endsWith('/objects/batch')
      appendFileSync(
        settings.records,
        `${JSON.stringify({ method: req.method, path: url.pathname, actor, ...(rejected ? { lfs_batch_rejected: true } : {}) })}\n`
      )
      const reply = (
        status: number,
        payload: Buffer | string = '',
        headers: Record<string, string> = {}
      ): void => {
        if (res.destroyed) return
        const content = Buffer.isBuffer(payload)
          ? payload
          : Buffer.from(payload)
        res.writeHead(status, {
          ...headers,
          'Content-Length': String(content.length),
        })
        res.end(content)
      }
      const json = (value: unknown, status = 200): void =>
        reply(status, JSON.stringify(value), {
          'Content-Type': 'application/vnd.git-lfs+json',
        })
      if (control.blockPath && url.pathname.includes(control.blockPath)) {
        writeFileSync(settings.started, 'started')
        const deadline = Date.now() + 25_000
        while (!existsSync(settings.release) && Date.now() < deadline)
          await new Promise(resolve => setTimeout(resolve, 20))
      }
      if (repository === undefined) return reply(404)
      if (actor !== control.allowed[repository])
        return reply(401, '', {
          'WWW-Authenticate': 'Basic realm="isolated Git credential fixture"',
        })
      const suffix = url.pathname.slice(repository.length)
      if (suffix.startsWith('/info/lfs')) {
        if (suffix.endsWith('/locks/verify'))
          return json({ ours: [], theirs: [], next_cursor: '' })
        if (suffix.endsWith('/objects/batch') && req.method === 'POST') {
          if (rejected)
            return json(
              {
                message: `fixture batch rejected ${settings.alice} Basic ${Buffer.from(`AliceCase:${settings.alice}`).toString('base64')}`,
              },
              422
            )
          const request: {
            operation: string
            objects: Array<{ oid: string; size: number }>
          } = JSON.parse(body.toString())
          const objects = request.objects.map(item => {
            if (!/^[a-f0-9]{64}$/.test(item.oid))
              throw new Error('Invalid object identifier')
            const present = existsSync(path.join(settings.objects, item.oid))
            const value: Record<string, unknown> = {
              ...item,
              authenticated: false,
            }
            const href = `https://${req.headers.host}${repository}/info/lfs/objects/${item.oid}`
            if (request.operation === 'upload' && !present)
              value.actions = { upload: { href } }
            else if (request.operation === 'download' && present)
              value.actions = { download: { href } }
            else if (request.operation === 'download')
              value.error = { code: 404, message: 'No fixture object' }
            return value
          })
          return json({ transfer: 'basic', objects })
        }
        if (suffix.startsWith('/info/lfs/objects/')) {
          const oid = suffix.slice(suffix.lastIndexOf('/') + 1)
          if (!/^[a-f0-9]{64}$/.test(oid)) return reply(404)
          const file = path.join(settings.objects, oid)
          if (req.method === 'PUT') {
            if (createHash('sha256').update(body).digest('hex') !== oid)
              throw new Error('Invalid object digest')
            writeFileSync(file, body)
            return reply(200)
          }
          if (req.method === 'GET' && existsSync(file))
            return reply(200, readFileSync(file), {
              'Content-Type': 'application/octet-stream',
            })
        }
        return reply(404)
      }
      const result = spawnSync(settings.backend, [], {
        input: body,
        timeout: 20_000,
        maxBuffer: 32 * 1024 * 1024,
        env: {
          ...process.env,
          GIT_PROJECT_ROOT: settings.projects,
          GIT_HTTP_EXPORT_ALL: '1',
          REQUEST_METHOD: req.method,
          PATH_INFO: url.pathname,
          QUERY_STRING: url.search.slice(1),
          CONTENT_TYPE: req.headers['content-type'] ?? '',
          CONTENT_LENGTH: String(body.length),
          REMOTE_USER: actor ?? '',
          REMOTE_ADDR: '127.0.0.1',
          SERVER_PROTOCOL: 'HTTP/1.1',
          ...(req.headers['git-protocol']
            ? { HTTP_GIT_PROTOCOL: String(req.headers['git-protocol']) }
            : {}),
        },
      })
      if (result.status !== 0) throw new Error('Git HTTP backend failed')
      let separator = result.stdout.indexOf('\r\n\r\n')
      let length = 4
      if (separator < 0) {
        separator = result.stdout.indexOf('\n\n')
        length = 2
      }
      if (separator < 0)
        throw new Error('Git HTTP backend returned no CGI header')
      let status = 200
      const headers: Record<string, string> = {}
      for (const line of result.stdout
        .subarray(0, separator)
        .toString('latin1')
        .split(/\r?\n/)) {
        const colon = line.indexOf(':')
        if (colon < 0) throw new Error('Malformed CGI header')
        const name = line.slice(0, colon)
        const value = line.slice(colon + 1).trim()
        if (name.toLowerCase() === 'status')
          status = Number(value.split(' ')[0])
        else if (
          !['content-length', 'transfer-encoding', 'connection'].includes(
            name.toLowerCase()
          )
        )
          headers[name] = value
      }
      reply(status, result.stdout.subarray(separator + length), headers)
    } catch (error) {
      if (!res.destroyed) {
        appendFileSync(
          settings.errors,
          `${error instanceof Error ? error.message : 'Unknown error'}\n`
        )
        res.writeHead(500).end()
      }
    }
  }
)
server.listen(0, '127.0.0.1', () => {
  const address = server.address()
  if (address === null || typeof address === 'string')
    throw new Error('Missing Git fixture address')
  parent.postMessage({ host: `127.0.0.1:${address.port}` })
})
