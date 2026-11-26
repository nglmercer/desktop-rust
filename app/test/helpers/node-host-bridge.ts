import { vi } from 'vitest'
import { spawn as nodeSpawn } from 'node:child_process'
import {
  closeSync,
  fstatSync,
  lstatSync,
  openSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
} from 'node:fs'
import { join } from 'node:path'

const LIMITS = {
  block: 64 * 1024,
  inputBudget: 1024 * 1024,
  outputWindow: 256 * 1024,
}

let lstatHook: ((path: string) => Promise<void> | void) | undefined

export function setNodeHostBridgeLstatHook(
  hook: ((path: string) => Promise<void> | void) | undefined
) {
  lstatHook = hook
}

let readRegularVerifiedHook: ((path: string) => void) | undefined

export function setNodeHostBridgeReadRegularVerifiedHook(
  hook: ((path: string) => void) | undefined
) {
  readRegularVerifiedHook = hook
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

function statResult(value: any) {
  const integer = (entry: number | bigint) =>
    typeof entry === 'bigint' ? Number(entry) : entry
  return {
    ...metadata(value),
    size: integer(value.size),
    mode: integer(value.mode),
    dev: integer(value.dev),
    ino: String(value.ino),
    nlink: integer(value.nlink),
    uid: integer(value.uid),
    gid: integer(value.gid),
    rdev: integer(value.rdev),
    blksize: integer(value.blksize),
    blocks: integer(value.blocks),
    mtimeMs: integer(value.mtimeMs),
    atimeMs: integer(value.atimeMs),
    birthtimeMs: integer(value.birthtimeMs),
    ctimeMs: integer(value.ctimeMs),
    mtimeNs: String(value.mtimeNs ?? 0),
    atimeNs: String(value.atimeNs ?? 0),
    birthtimeNs: String(value.birthtimeNs ?? 0),
    ctimeNs: String(value.ctimeNs ?? 0),
  }
}

/**
 * Node-backed implementation of the host filesystem/process bridge. It lets
 * tests exercise the real web adapters end-to-end while production uses Rust.
 */
const listeners = new Map<string, (event: unknown, message: any) => void>()
let nextProcessId = 1

export function installNodeHostBridge() {
  const children = new Map<number, INodeChild>()
  const fileHandles = new Map<number, import('fs/promises').FileHandle>()
  const gitCancellations = new Map<
    number,
    { child?: ReturnType<typeof nodeSpawn>; cancelled: boolean }
  >()
  let nextFileHandleId = 1

  const emit = (message: Record<string, unknown>) => {
    listeners.get('desktop-process')?.(null, message)
  }

  const fsInvoke = async (
    operation: string,
    options: any
  ): Promise<unknown> => {
    const fs = await import('fs/promises')
    switch (operation) {
      case 'write-file': {
        const bytes = Buffer.from(options.data ?? '', 'base64')
        if (options.append === true) {
          await fs.writeFile(options.path, bytes, {
            flag: options.exclusive === true ? 'ax' : 'a',
            mode: options.mode,
          })
        } else {
          await fs.writeFile(options.path, bytes, {
            flag: options.exclusive === true ? 'wx' : 'w',
            mode: options.mode,
          })
        }
        return null
      }
      case 'open': {
        const handle = await fs.open(options.path, options.flags, options.mode)
        const id = nextFileHandleId++
        fileHandles.set(id, handle)
        return { id }
      }
      case 'handle-stat': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        return statResult(await handle.stat({ bigint: true }))
      }
      case 'handle-read-file': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        return (await handle.readFile()).toString('base64')
      }
      case 'handle-write-file': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        await handle.writeFile(Buffer.from(options.data ?? '', 'base64'))
        return null
      }
      case 'handle-chmod': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        await handle.chmod(options.mode)
        return null
      }
      case 'handle-utimes': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        await handle.utimes(options.atime, options.mtime)
        return null
      }
      case 'handle-sync': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        await handle.sync()
        return null
      }
      case 'handle-close': {
        const handle = fileHandles.get(options.id)
        if (handle === undefined) {
          throw Object.assign(new Error('EBADF: bad file descriptor'), {
            code: 'EBADF',
          })
        }
        await handle.close()
        fileHandles.delete(options.id)
        return null
      }
      case 'read-file': {
        return (await fs.readFile(options.path)).toString('base64')
      }
      case 'realpath':
        return fs.realpath(options.path)
      case 'readlink': {
        const bytes = await fs.readlink(options.path, { encoding: 'buffer' })
        return {
          target: bytes.toString('utf8'),
          target_base64: bytes.toString('base64'),
        }
      }
      case 'chmod':
        await fs.chmod(options.path, options.mode)
        return null
      case 'lstat': {
        const result = statResult(
          await fs.lstat(options.path, { bigint: true })
        )
        await lstatHook?.(options.path)
        return result
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
      case 'rename':
        await fs.rename(options.path, options.to)
        return null
      case 'unlink':
        await fs.unlink(options.path)
        return null
      case 'rm':
        await fs.rm(options.path, {
          recursive: options.recursive === true,
          force: options.force === true,
        })
        return null
      case 'mkdir':
        await fs.mkdir(options.path, {
          recursive: options.recursive === true,
        })
        return null
      case 'copy-file':
        await fs.cp(options.path, options.to)
        return null
      case 'copy':
        await fs.cp(options.path, options.to, { recursive: true })
        return null
      case 'mkdtemp':
        return fs.mkdtemp(options.prefix)
      case 'access':
        await fs.access(options.path, options.mode)
        return null
      case 'stat': {
        return statResult(await fs.stat(options.path, { bigint: true }))
      }
      case 'readdir':
        return (await fs.readdir(options.path, { withFileTypes: true })).map(
          entry => ({ name: entry.name, ...metadata(entry) })
        )
      default:
        throw new Error(`unexpected fs ${operation}`)
    }
  }

  const gitExec = (options: any): Promise<unknown> =>
    new Promise((resolve, reject) => {
      const cancelId = options.cancelId
      const cancellation =
        typeof cancelId === 'number'
          ? (gitCancellations.get(cancelId) ??
            (() => {
              const created: {
                child?: ReturnType<typeof nodeSpawn>
                cancelled: boolean
              } = { cancelled: false }
              gitCancellations.set(cancelId, created)
              return created
            })())
          : undefined
      if (cancellation?.cancelled === true) {
        reject(
          Object.assign(new Error('The operation was aborted.'), {
            code: 'ABORT_ERR',
          })
        )
        return
      }
      const env: Record<string, string> = {}
      for (const [key, value] of Object.entries(process.env)) {
        if (value !== undefined) {
          env[key] = value
        }
      }
      env.GIT_TERMINAL_PROMPT = '0'
      env.LANGUAGE = 'en'
      for (const [key, value] of Object.entries(options.env ?? {})) {
        if (value === null || value === undefined) {
          delete env[key]
        } else {
          env[key] = String(value)
        }
      }
      let child: ReturnType<typeof nodeSpawn>
      try {
        const git =
          process.env.LOCAL_GIT_DIRECTORY === undefined ||
          process.env.LOCAL_GIT_DIRECTORY === ''
            ? 'git'
            : join(process.env.LOCAL_GIT_DIRECTORY, 'bin', 'git')
        child = nodeSpawn(git, options.args ?? [], {
          cwd: options.cwd || undefined,
          env,
          stdio: [
            options.stdin === undefined ? 'ignore' : 'pipe',
            'pipe',
            'pipe',
          ],
        })
      } catch (error) {
        reject(error)
        return
      }
      if (cancellation !== undefined) {
        cancellation.child = child
      }
      const stdout: Buffer[] = []
      const stderr: Buffer[] = []
      let stdoutBytes = 0
      let stderrBytes = 0
      let stopped = false
      const stopForOutput = () => {
        if (!stopped) {
          stopped = true
          child.kill()
        }
      }
      child.stdout?.on('data', chunk => {
        stdoutBytes += chunk.byteLength
        if (
          options.maxBuffer !== undefined &&
          stdoutBytes > options.maxBuffer
        ) {
          stopForOutput()
          return
        }
        stdout.push(Buffer.from(chunk))
      })
      child.stderr?.on('data', chunk => {
        stderrBytes += chunk.byteLength
        if (
          options.maxBuffer !== undefined &&
          stderrBytes > options.maxBuffer
        ) {
          stopForOutput()
          return
        }
        stderr.push(Buffer.from(chunk))
      })
      child.on('error', error => reject(error))
      child.on('close', code => {
        if (cancelId !== undefined) {
          gitCancellations.delete(cancelId)
        }
        const binary = options.encoding === 'buffer'
        const encode = (bytes: Buffer) =>
          binary ? bytes.toString('base64') : bytes.toString('utf8')
        if (cancellation?.cancelled === true) {
          reject(
            Object.assign(new Error('The operation was aborted.'), {
              code: 'ABORT_ERR',
            })
          )
          return
        }
        if (stopped) {
          const error = Object.assign(
            new Error('stdout maxBuffer length exceeded'),
            {
              code: 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER',
              details: {
                stdout: encode(Buffer.concat(stdout)),
                stderr: encode(Buffer.concat(stderr)),
                truncated: true,
              },
            }
          )
          reject(error)
          return
        }
        resolve({
          stdout: encode(Buffer.concat(stdout)),
          stderr: encode(Buffer.concat(stderr)),
          exitCode: code ?? 1,
        })
      })
      if (options.stdin !== undefined) {
        child.stdin?.end(Buffer.from(options.stdin, 'base64'))
      }
    })

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
        case 'desktop.io.cancel': {
          const cancellation = gitCancellations.get(options.cancelId)
          if (cancellation === undefined) {
            gitCancellations.set(options.cancelId, { cancelled: true })
          } else {
            cancellation.cancelled = true
            cancellation.child?.kill()
          }
          return Promise.resolve(null)
        }
        case 'desktop.io.git-exec':
          return gitExec(options)
        case 'desktop.io.process-spawn': {
          const id = nextProcessId++
          const stdio = (options.stdio ?? ['pipe', 'pipe', 'pipe']).map(
            (mode: string) => (mode === 'ignore' ? 'ignore' : 'pipe')
          )
          let env: Record<string, string | undefined> | undefined
          if (options.useGitBinary === true) {
            env = {
              ...process.env,
              GIT_TERMINAL_PROMPT: '0',
              LANGUAGE: 'en',
            }
            for (const [key, value] of Object.entries(
              (options.env ?? {}) as Record<string, string | null>
            )) {
              if (
                key === 'DESKTOP_PORT' ||
                key === 'DESKTOP_TRAMPOLINE_TOKEN' ||
                key === 'GIT_ASKPASS' ||
                key === 'SSH_ASKPASS' ||
                key === 'LOCAL_GIT_DIRECTORY' ||
                key === 'GIT_EXEC_PATH' ||
                key === 'GIT_TEMPLATE_DIR'
              ) {
                continue
              }
              if (value === null || value === undefined) {
                delete env[key]
              } else {
                env[key] = String(value)
              }
            }
          } else {
            const envEntries =
              options.env === undefined
                ? undefined
                : (Object.entries(
                    options.env as Record<string, string | null>
                  ).filter(
                    ([, value]) => value !== null && value !== undefined
                  ) as Array<[string, string]>)
            env =
              envEntries === undefined
                ? undefined
                : Object.fromEntries(envEntries)
          }
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
          let startupSettled = false
          let startupError: Error | undefined
          child.on('error', error => {
            if (!startupSettled) {
              startupError = error
              return
            }
            emit({ id, event: 'end', stream: 'stdout' })
            emit({ id, event: 'end', stream: 'stderr' })
            emit({ id, event: 'exit', code: 1, signal: null })
          })
          child.on('close', (code, signal) => {
            emit({ id, event: 'exit', code, signal })
            children.delete(id)
          })
          return new Promise((resolve, reject) => {
            setImmediate(() => {
              startupSettled = true
              if (startupError !== undefined) {
                children.delete(id)
                reject(startupError)
              } else {
                resolve({ id, pid: child.pid, limits: LIMITS })
              }
            })
          })
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
    invokeSync(channel: string, options: any): unknown {
      if (channel === 'desktop.io.fs-lstat') {
        return statResult(lstatSync(options.path, { bigint: true }))
      }
      if (channel === 'desktop.io.fs-stat') {
        return statResult(statSync(options.path, { bigint: true }))
      }
      if (channel === 'desktop.io.fs-read-regular-verified') {
        const descriptor = openSync(options.path, 'r')
        try {
          const held = fstatSync(descriptor, { bigint: true })
          if (!held.isFile()) {
            return {
              data: '',
              physical: null,
              stat: statResult(held),
              target: null,
              matches: false,
            }
          }
          if (held.size > BigInt(options.maxBytes)) {
            throw Object.assign(new Error('EFBIG: file too large'), {
              code: 'EFBIG',
            })
          }
          readRegularVerifiedHook?.(options.path)
          const physical = realpathSync.native(options.path)
          const bytes = readFileSync(descriptor)
          if (bytes.byteLength > options.maxBytes) {
            throw Object.assign(new Error('EFBIG: file too large'), {
              code: 'EFBIG',
            })
          }
          const target = lstatSync(physical, { bigint: true })
          return {
            data: bytes.toString('base64'),
            physical,
            stat: statResult(held),
            target: statResult(target),
            matches: target.dev === held.dev && target.ino === held.ino,
          }
        } finally {
          closeSync(descriptor)
        }
      }
      if (channel === 'desktop.io.fs-realpath') {
        return realpathSync.native(options.path)
      }
      if (channel === 'desktop.io.fs-readdir') {
        return readdirSync(options.path, { withFileTypes: true }).map(
          entry => ({
            name: entry.name,
            ...metadata(entry),
          })
        )
      }
      throw new Error(`unexpected sync ${channel}`)
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
