/** Isolated TLS API and CONNECT proxy for pull-request creation tests. */
import {
  appendFileSync,
  existsSync,
  readFileSync,
  writeFileSync,
} from 'node:fs'
import { createServer as createHttpServer } from 'node:http'
import { createServer as createHttpsServer } from 'node:https'
import { connect } from 'node:net'
import type { Duplex } from 'node:stream'
import { parentPort, workerData } from 'node:worker_threads'

interface IWorkerSettings {
  readonly cert: string
  readonly key: string
  readonly scenario: string
  readonly control: string
  readonly records: string
  readonly errors: string
  readonly started: string
  readonly release: string
}
interface IUser {
  readonly id: number
  readonly login: string
  readonly token: string
}
interface IScenario {
  readonly users: Record<string, IUser>
  readonly environment: IUser
  readonly repositories: Record<string, unknown>
  readonly repository_responses?: Record<string, unknown>
}
interface IControl {
  readonly failures: Record<string, string>
  readonly url: string | null
  readonly node: Record<string, unknown> | null
  readonly pages: ReadonlyArray<ReadonlyArray<unknown>>
  readonly page_override?: unknown
  readonly gate?: string
}
const settings: IWorkerSettings = workerData
if (parentPort === null) throw new Error('API worker requires a parent')
const parent = parentPort
const sockets = new Set<Duplex>()
const reportError = (message: string): void => {
  appendFileSync(settings.errors, `${message}\n`)
}
const api = createHttpsServer(
  { cert: readFileSync(settings.cert), key: readFileSync(settings.key) },
  async (req, res) => {
    try {
      const state: IScenario = JSON.parse(
        readFileSync(settings.scenario, 'utf8')
      )
      const control: IControl = JSON.parse(
        readFileSync(settings.control, 'utf8')
      )
      const authorization = (req.headers.authorization ?? '').split(' ', 2)
      const supplied = ['bearer', 'token'].includes(
        (authorization[0] ?? '').toLowerCase()
      )
        ? authorization[1]
        : ''
      const actor = [...Object.values(state.users), state.environment].find(
        user => user.token === supplied
      )
      const chunks: Buffer[] = []
      let size = 0
      for await (const chunk of req) {
        const bytes = Buffer.from(chunk)
        size += bytes.length
        if (size > 2 * 1024 * 1024) throw new Error('request too large')
        chunks.push(bytes)
      }
      const payload: {
        query?: string
        variables?: { after?: string | null }
      } | null = size ? JSON.parse(Buffer.concat(chunks).toString()) : null
      const query = payload?.query ?? ''
      let route = new URL(req.url ?? '/', 'https://localhost').pathname.replace(
        /^\/api\/v3/,
        ''
      )
      if (route === '/api/graphql') route = '/graphql'
      const stage = query.includes('createPullRequest')
        ? 'mutation'
        : route === '/graphql'
          ? 'lookup'
          : route === '/user'
            ? 'user'
            : 'repository'
      const record = {
        method: req.method,
        path: route,
        stage,
        actor: actor?.login ?? null,
        payload,
      }
      const serialized = JSON.stringify(record)
      if (
        [...Object.values(state.users), state.environment].some(user =>
          serialized.includes(user.token)
        )
      )
        throw new Error('credential in API log')
      appendFileSync(settings.records, `${serialized}\n`)
      const reply = (value: unknown, status = 200): void => {
        if (res.destroyed) return
        res.writeHead(status, { 'Content-Type': 'application/json' })
        res.end(Buffer.isBuffer(value) ? value : JSON.stringify(value))
      }
      if (actor === undefined) {
        reply({ message: 'fixture authentication required' }, 401)
        return
      }
      if (control.gate === stage) {
        writeFileSync(settings.started, 'started')
        const deadline = Date.now() + 25_000
        while (!existsSync(settings.release) && Date.now() < deadline)
          await new Promise(resolve => setTimeout(resolve, 20))
        if (!existsSync(settings.release))
          throw new Error('response gate timed out')
      }
      const failure = control.failures[stage]
      if (failure === 'http') {
        reply(
          { message: `fixture rejected ${state.users.AliceCase.token}` },
          422
        )
        return
      }
      if (failure === 'malformed') {
        reply(Buffer.from(`{bad ${state.users.AliceCase.token}`))
        return
      }
      if (failure === 'graphql') {
        reply({
          errors: [
            {
              message: `fixture GraphQL rejected ${state.users.AliceCase.token}`,
            },
          ],
        })
        return
      }
      if (stage === 'user') reply({ id: actor.id, login: actor.login })
      else if (stage === 'repository')
        reply(
          state.repository_responses?.[route] ?? state.repositories[actor.login]
        )
      else if (stage === 'lookup') {
        const after = payload?.variables?.after
        const page = after == null ? 0 : Number(after.replace(/^cursor-/, ''))
        const node =
          control.node === null ? null : structuredClone(control.node)
        if (node !== null)
          node.pullRequests = control.page_override ?? {
            nodes: control.pages[page] ?? [],
            pageInfo: {
              hasNextPage: page + 1 < control.pages.length,
              endCursor: `cursor-${page + 1}`,
            },
          }
        reply({ data: { repository: node } })
      } else if (stage === 'mutation')
        reply({
          data: {
            createPullRequest: {
              pullRequest: { id: 'PR_fixture', url: control.url },
            },
          },
        })
      else reply({ message: 'unknown isolated API request' }, 404)
    } catch (error) {
      if (!res.destroyed) {
        reportError(error instanceof Error ? error.name : 'Unknown error')
        res.writeHead(500).end()
      }
    }
  }
)
api.on('tlsClientError', error => {
  const code = 'code' in error ? error.code : undefined
  if (
    code !== 'ERR_SSL_UNEXPECTED_EOF_WHILE_READING' &&
    code !== 'ECONNRESET' &&
    code !== 'EPIPE'
  ) {
    reportError('unexpected TLS read failure')
  }
})
const proxy = createHttpServer((_req, res) => {
  res.writeHead(403).end()
})
proxy.on('connect', (req, socket, head) => {
  if (req.url !== 'api.github.com:443') {
    reportError('unexpected API proxy target')
    socket.end('HTTP/1.1 403 Forbidden\r\n\r\n')
    return
  }
  const address = api.address()
  if (address === null || typeof address === 'string') {
    socket.destroy()
    return
  }
  const upstream = connect(address.port, '127.0.0.1', () => {
    socket.write('HTTP/1.1 200 Connection established\r\n\r\n')
    if (head.length) upstream.write(head)
    upstream.pipe(socket)
    socket.pipe(upstream)
  })
  socket.on('error', () => upstream.destroy())
  upstream.on('error', () => socket.destroy())
  sockets.add(socket)
  sockets.add(upstream)
  socket.once('close', () => sockets.delete(socket))
  upstream.once('close', () => sockets.delete(upstream))
})
api.listen(0, '127.0.0.1', () =>
  proxy.listen(0, '127.0.0.1', () => {
    const a = api.address()
    const p = proxy.address()
    if (
      a === null ||
      p === null ||
      typeof a === 'string' ||
      typeof p === 'string'
    )
      throw new Error('Missing fixture address')
    parent.postMessage({
      endpoint: `https://127.0.0.1:${a.port}`,
      proxy: `http://127.0.0.1:${p.port}`,
    })
  })
)
parent.on('message', message => {
  if (message !== 'close') return
  for (const socket of sockets) socket.destroy()
  api.closeAllConnections()
  proxy.closeAllConnections()
  api.close()
  proxy.close()
  parent.close()
})
