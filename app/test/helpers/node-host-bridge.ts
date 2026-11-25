import { vi } from 'vitest'
import { spawn as nodeSpawn } from 'node:child_process'
import {
  access as accessAsync,
  readdir as readdirAsync,
  stat as statAsync,
} from 'node:fs/promises'
import {
  appendFileSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'

const LIMITS = {
  block: 64 * 1024,
  inputBudget: 1024 * 1024,
  outputWindow: 256 * 1024,
}

interface INodeChild {
  readonly child: ReturnType<typeof nodeSpawn>
  stdoutSeq: number
  stderrSeq: number
}

function metadata(value: {
  isFile(): boolean
  isDirectory(): boolean
  isSymbolicLink(): boolean
}) {
  return {
    isFile: value.isFile(),
    isDirectory: value.isDirectory(),
    isSymbolicLink: value.isSymbolicLink(),
  }
}

/**
 * Node-backed implementation of the host filesystem/process bridge. It lets
 * tests exercise the real web adapters end-to-end while production uses Rust.
 */
const listeners = new Map<string, (event: unknown, message: any) => void>()

export function installNodeHostBridge() {
  const children = new Map<number, INodeChild>()
  let nextId = 1

  const emit = (message: Record<string, unknown>) => {
    listeners.get('desktop-process')?.(null, message)
  }

  const fsInvoke = async (
    operation: string,
    options: any
  ): Promise<unknown> => {
    switch (operation) {
      case 'write-file': {
        const bytes = Buffer.from(options.data ?? '', 'base64')
        if (options.append === true) {
          appendFileSync(options.path, bytes)
        } else {
          writeFileSync(options.path, bytes, {
            flag: options.exclusive === true ? 'wx' : 'w',
            mode: options.mode,
          })
        }
        return null
      }
      case 'read-range': {
        const bytes = readFileSync(options.path)
        const stat = statSync(options.path)
        const part = bytes.subarray(
          options.offset,
          options.offset + options.length
        )
        return {
          data: part.toString('base64'),
          bytesRead: part.byteLength,
          eof: options.offset + part.byteLength >= bytes.byteLength,
          identity: {
            dev: stat.dev,
            ino: String(stat.ino),
            size: stat.size,
            mtimeMs: stat.mtimeMs,
          },
        }
      }
      case 'rm':
        rmSync(options.path, {
          recursive: options.recursive === true,
          force: options.force === true,
        })
        return null
      case 'mkdir':
        mkdirSync(options.path, { recursive: options.recursive === true })
        return null
      case 'copy-file':
        copyFileSync(options.path, options.to)
        return null
      case 'mkdtemp':
        return mkdtempSync(options.prefix)
      case 'access':
        await accessAsync(options.path, options.mode)
        return null
      case 'stat': {
        const value = await statAsync(options.path)
        return {
          ...metadata(value),
          size: value.size,
          mode: value.mode,
          dev: value.dev,
          ino: String(value.ino),
          mtimeMs: value.mtimeMs,
          atimeMs: value.atimeMs,
          birthtimeMs: value.birthtimeMs,
          ctimeMs: value.ctimeMs,
        }
      }
      case 'readdir':
        return (await readdirAsync(options.path, { withFileTypes: true })).map(
          entry => ({ name: entry.name, ...metadata(entry) })
        )
      default:
        throw new Error(`unexpected fs ${operation}`)
    }
  }

  const bridge = {
    invoke(channel: string, options: any): Promise<unknown> {
      if (channel.startsWith('desktop.io.fs-')) {
        try {
          return Promise.resolve(
            fsInvoke(channel.slice('desktop.io.fs-'.length), options)
          )
        } catch (error) {
          return Promise.reject(error)
        }
      }

      switch (channel) {
        case 'desktop.io.process-spawn': {
          const id = nextId++
          const stdio = (options.stdio ?? ['pipe', 'pipe', 'pipe']).map(
            (mode: string) => (mode === 'ignore' ? 'ignore' : 'pipe')
          )
          const envEntries =
            options.env === undefined
              ? undefined
              : (Object.entries(
                  options.env as Record<string, string | null>
                ).filter(
                  ([, value]) => value !== null && value !== undefined
                ) as Array<[string, string]>)
          const env =
            envEntries === undefined
              ? undefined
              : Object.fromEntries(envEntries)
          let child: ReturnType<typeof nodeSpawn>
          try {
            child = nodeSpawn(options.command, options.args ?? [], {
              cwd: options.cwd || undefined,
              env,
              stdio,
              detached: options.detached === true,
            })
          } catch (error) {
            return Promise.reject(error)
          }
          child.stdin?.on('error', () => {})
          const entry: INodeChild = { child, stdoutSeq: 0, stderrSeq: 0 }
          children.set(id, entry)
          const wire = (stream: 'stdout' | 'stderr') => {
            const pipe = child[stream]
            if (pipe === null) return
            pipe.on('data', chunk => {
              const seq =
                stream === 'stdout' ? ++entry.stdoutSeq : ++entry.stderrSeq
              emit({
                id,
                event: 'data',
                stream,
                seq,
                data: Buffer.from(chunk).toString('base64'),
              })
            })
            pipe.on('end', () => emit({ id, event: 'end', stream }))
          }
          wire('stdout')
          wire('stderr')
          child.on('error', error => {
            emit({ id, event: 'end', stream: 'stdout' })
            emit({ id, event: 'end', stream: 'stderr' })
            emit({ id, event: 'exit', code: 1, signal: null })
          })
          child.on('close', (code, signal) => {
            emit({ id, event: 'exit', code, signal })
            children.delete(id)
          })
          return Promise.resolve({ id, pid: child.pid, limits: LIMITS })
        }
        case 'desktop.io.process-write': {
          const entry = children.get(options.id)
          if (entry?.child.stdin === null || entry?.child.stdin === undefined) {
            return Promise.reject(
              Object.assign(new Error('process stdin is unavailable'), {
                code: 'EPIPE',
              })
            )
          }
          return new Promise((resolve, reject) => {
            entry.child.stdin!.write(
              Buffer.from(options.data, 'base64'),
              error => (error ? reject(error) : resolve(null))
            )
          })
        }
        case 'desktop.io.process-end': {
          const entry = children.get(options.id)
          entry?.child.stdin?.end()
          return Promise.resolve(null)
        }
        case 'desktop.io.process-kill': {
          const entry = children.get(options.id)
          entry?.child.kill(options.signal ?? 'SIGTERM')
          return Promise.resolve(null)
        }
        default:
          return Promise.reject(new Error(`unexpected ${channel}`))
      }
    },
    invokeSync: () => {
      throw new Error('synchronous host bridge is unavailable in this test')
    },
    send: () => {},
    on(channel: string, listener: (event: unknown, message: any) => void) {
      listeners.set(channel, listener)
      return bridge
    },
    removeListener: () => bridge,
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  }
  vi.stubGlobal('__desktop', bridge)
}

export function uninstallNodeHostBridge() {
  vi.unstubAllGlobals()
}
