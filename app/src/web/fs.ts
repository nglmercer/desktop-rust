// `fs` for the webview build. Every call is forwarded to the Rust shell's
// `desktop.io.fs-*` commands: asynchronous ones over the IPC bridge, the
// `*Sync` ones over a blocking request (see `invokeSync` in ipc-shim.js).
//
// File handles are emulated by path. There is no real descriptor behind
// them, so a handle does not pin an inode the way Node's does: code that
// relies on that for locking gets best-effort behaviour here.

import {
  bundleUrl,
  bundleEntry,
  bundleEntrySync,
  IBundleEntry,
} from './bundle'

type Encoding = BufferEncoding | null | undefined
type ReadOptions = Encoding | { encoding?: Encoding; flag?: string }
type PathLike = string | URL | Buffer

interface IRawStats {
  isFile: boolean
  isDirectory: boolean
  isSymbolicLink: boolean
  size: number
  mode: number
  dev: number
  ino: string
  mtimeMs: number
  atimeMs: number
  birthtimeMs: number
  ctimeMs: number
}

interface IRawDirent {
  name: string
  isFile: boolean
  isDirectory: boolean
  isSymbolicLink: boolean
}

interface IBridge {
  invoke(channel: string, options: object): Promise<any>
  invokeSync(channel: string, options: object): any
  encodeBase64(bytes: Uint8Array): string
  decodeBase64(text: string): Uint8Array
}

function bridge(): IBridge {
  const b = (globalThis as any).__desktop
  if (b === undefined || typeof b.invokeSync !== 'function') {
    throw Object.assign(
      new Error('The file system is only available inside the desktop shell'),
      { code: 'ENOSYS' }
    )
  }
  return b
}

// The system call Node names in the error for each operation.
const syscalls: Record<string, string> = {
  'read-file': 'open',
  'write-file': 'open',
  readdir: 'scandir',
  'copy-file': 'copyfile',
  copy: 'cp',
}

/**
 * Give a bridge failure the fields of a Node system error. Callers tell
 * those apart from other exceptions by `code` and `syscall` together.
 */
function nodeError(error: any, op: string, options: any): never {
  if (error && typeof error.code === 'string') {
    error.syscall ??= syscalls[op] ?? op
    error.path ??= options?.path
    error.errno ??= -1
  }
  throw error
}

const call = (op: string, options: object) =>
  bridge()
    .invoke(`desktop.io.fs-${op}`, options)
    .catch(error => nodeError(error, op, options))
function callSync(op: string, options: object) {
  try {
    return bridge().invokeSync(`desktop.io.fs-${op}`, options)
  } catch (error) {
    return nodeError(error, op, options)
  }
}

function toPath(path: PathLike): string {
  if (typeof path === 'string') {
    return path
  }
  if (path instanceof URL) {
    return decodeURIComponent(path.pathname)
  }
  return path.toString()
}

function toBytes(data: unknown, encoding?: Encoding): Uint8Array {
  if (typeof data === 'string') {
    return Buffer.from(data, encoding ?? 'utf8')
  }
  if (data instanceof Uint8Array) {
    return data
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
  }
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data)
  }
  return Buffer.from(String(data))
}

const encodingOf = (options: ReadOptions): Encoding =>
  typeof options === 'string' ? options : options?.encoding

function decode(base64: string, options: ReadOptions): any {
  const buffer = Buffer.from(bridge().decodeBase64(base64))
  const encoding = encodingOf(options)
  return encoding ? buffer.toString(encoding) : buffer
}

export class Stats {
  public readonly size: number
  public readonly mode: number
  public readonly dev: number | bigint
  public readonly ino: number | bigint
  public readonly mtimeMs: number
  public readonly atimeMs: number
  public readonly ctimeMs: number
  public readonly birthtimeMs: number
  public readonly mtime: Date
  public readonly atime: Date
  public readonly ctime: Date
  public readonly birthtime: Date

  public constructor(private readonly raw: IRawStats, bigint = false) {
    this.size = raw.size
    this.mode = raw.mode
    this.dev = bigint ? BigInt(raw.dev) : raw.dev
    this.ino = bigint ? BigInt(raw.ino) : Number(raw.ino)
    this.mtimeMs = raw.mtimeMs
    this.atimeMs = raw.atimeMs
    this.ctimeMs = raw.ctimeMs
    this.birthtimeMs = raw.birthtimeMs
    this.mtime = new Date(raw.mtimeMs)
    this.atime = new Date(raw.atimeMs)
    this.ctime = new Date(raw.ctimeMs)
    this.birthtime = new Date(raw.birthtimeMs)
  }

  public isFile() {
    return this.raw.isFile
  }
  public isDirectory() {
    return this.raw.isDirectory
  }
  public isSymbolicLink() {
    return this.raw.isSymbolicLink
  }
  public isFIFO() {
    return false
  }
  public isSocket() {
    return false
  }
  public isBlockDevice() {
    return false
  }
  public isCharacterDevice() {
    return false
  }
}
export class BigIntStats extends Stats {}

export class Dirent {
  public readonly name: string
  public readonly parentPath: string
  public readonly path: string

  public constructor(private readonly raw: IRawDirent, parent: string) {
    this.name = raw.name
    this.parentPath = parent
    this.path = parent
  }
  public isFile() {
    return this.raw.isFile
  }
  public isDirectory() {
    return this.raw.isDirectory
  }
  public isSymbolicLink() {
    return this.raw.isSymbolicLink
  }
}

// Linux values; the renderer only combines and passes them back to us.
export const constants = {
  F_OK: 0,
  X_OK: 1,
  W_OK: 2,
  R_OK: 4,
  O_RDONLY: 0,
  O_WRONLY: 1,
  O_RDWR: 2,
  O_CREAT: 0o100,
  O_EXCL: 0o200,
  O_TRUNC: 0o1000,
  O_APPEND: 0o2000,
  O_NONBLOCK: 0o4000,
  O_NOFOLLOW: 0o400000,
  COPYFILE_EXCL: 1,
}

interface IOpenMode {
  readonly write: boolean
  readonly create: boolean
  readonly exclusive: boolean
  readonly truncate: boolean
  readonly append: boolean
}

function openMode(flags: string | number | undefined): IOpenMode {
  if (typeof flags === 'number') {
    return {
      write: (flags & 3) !== constants.O_RDONLY,
      create: (flags & constants.O_CREAT) !== 0,
      exclusive: (flags & constants.O_EXCL) !== 0,
      truncate: (flags & constants.O_TRUNC) !== 0,
      append: (flags & constants.O_APPEND) !== 0,
    }
  }
  const f = flags ?? 'r'
  return {
    write: /[wa+]/.test(f),
    create: /[wa]/.test(f),
    exclusive: f.includes('x'),
    truncate: f.includes('w'),
    append: f.includes('a'),
  }
}

const statOptions = (o: any) => o?.bigint === true

// --- bundled resources -------------------------------------------------------
// Files that shipped inside the Electron app directory (emoji.json, static/)
// are served by the shell from the page's origin. They are read-only.

async function readBundled(url: string, path: string): Promise<Buffer> {
  const response = await fetch(url)
  if (!response.ok) {
    throw missing(path)
  }
  return Buffer.from(await response.arrayBuffer())
}

function readBundledSync(url: string, path: string): Buffer {
  const request = new XMLHttpRequest()
  request.open('GET', url, false)
  // Keep the bytes intact: one char per byte, no charset decoding.
  request.overrideMimeType('text/plain; charset=x-user-defined')
  request.send()
  if (request.status < 200 || request.status >= 300) {
    throw missing(path)
  }
  const text = request.responseText
  const bytes = Buffer.alloc(text.length)
  for (let i = 0; i < text.length; i++) {
    bytes[i] = text.charCodeAt(i) & 0xff
  }
  return bytes
}

const missing = (path: string) =>
  Object.assign(new Error(`ENOENT: no such file or directory, open '${path}'`), {
    code: 'ENOENT',
    syscall: 'open',
    errno: -2,
    path,
  })

const isBundled = (path: PathLike) => bundleUrl(toPath(path)) !== null

function bundledStats(entry: IBundleEntry | null, path: string, options: any) {
  if (entry === null) {
    throw missing(path)
  }
  const raw: IRawStats = {
    isFile: entry.kind === 'file',
    isDirectory: entry.kind === 'directory',
    isSymbolicLink: false,
    size: entry.size ?? 0,
    mode: entry.kind === 'file' ? 0o100444 : 0o40555,
    dev: 0,
    ino: '0',
    mtimeMs: 0,
    atimeMs: 0,
    birthtimeMs: 0,
    ctimeMs: 0,
  }
  return new Stats(raw, statOptions(options))
}

function bundledListing(entry: IBundleEntry | null, path: string): IRawDirent[] {
  if (entry === null || entry.entries === undefined) {
    throw missing(path)
  }
  return entry.entries.map(e => ({
    name: e.name,
    isFile: !e.isDirectory,
    isDirectory: e.isDirectory,
    isSymbolicLink: false,
  }))
}

const withEncoding = (buffer: Buffer, options: ReadOptions) => {
  const encoding = encodingOf(options)
  return encoding ? buffer.toString(encoding) : buffer
}

// --- shared argument builders ------------------------------------------------

function writeArgs(path: PathLike, data: unknown, options: any, append: boolean) {
  const encoding = typeof options === 'string' ? options : options?.encoding
  const flag: string = (typeof options === 'object' && options?.flag) || ''
  return {
    path: toPath(path),
    data: bridge().encodeBase64(toBytes(data, encoding)),
    append: append || flag.includes('a'),
    exclusive: flag.includes('x'),
    mode: typeof options === 'object' ? options?.mode : undefined,
  }
}

const mkdirArgs = (path: PathLike, options: any) => ({
  path: toPath(path),
  recursive: typeof options === 'object' && options?.recursive === true,
})

const rmArgs = (path: PathLike, options: any) => ({
  path: toPath(path),
  recursive: options?.recursive === true,
  force: options?.force === true,
})

function direntResult(path: PathLike, options: any, raw: IRawDirent[]) {
  return options?.withFileTypes === true
    ? raw.map(entry => new Dirent(entry, toPath(path)))
    : raw.map(entry => entry.name)
}

// --- promises ----------------------------------------------------------------

export class FileHandle {
  private closed = false

  public constructor(
    private readonly path: string,
    private readonly mode: IOpenMode
  ) {}

  public async stat(options?: any) {
    return new Stats(await call('stat', { path: this.path }), statOptions(options))
  }
  public async readFile(options?: ReadOptions) {
    return decode(await call('read-file', { path: this.path }), options)
  }
  public async read(
    buffer: Uint8Array,
    offset = 0,
    length = buffer.byteLength - offset,
    position: number | null = 0
  ) {
    const bytes: Buffer = await this.readFile()
    const start = position ?? 0
    const chunk = bytes.subarray(start, start + length)
    buffer.set(chunk, offset)
    return { bytesRead: chunk.byteLength, buffer }
  }
  public async writeFile(data: unknown, options?: any) {
    await call('write-file', writeArgs(this.path, data, options, this.mode.append))
  }
  public async appendFile(data: unknown, options?: any) {
    await call('write-file', writeArgs(this.path, data, options, true))
  }
  public async write(data: unknown) {
    const bytes = toBytes(data)
    await call('write-file', writeArgs(this.path, bytes, undefined, true))
    return { bytesWritten: bytes.byteLength, buffer: data }
  }
  public async truncate() {
    await call('write-file', writeArgs(this.path, new Uint8Array(0), undefined, false))
  }
  public async chmod(mode: number) {
    await call('chmod', { path: this.path, mode })
  }
  public async utimes() {
    // Timestamps cannot be set through the bridge yet.
  }
  public async sync() {}
  public async datasync() {}
  public async close() {
    this.closed = true
  }
  public async [Symbol.asyncDispose]() {
    await this.close()
  }
  public get isClosed() {
    return this.closed
  }
}

async function open(path: PathLike, flags?: string | number, mode?: number) {
  const target = toPath(path)
  const how = openMode(flags)
  if (how.create) {
    if (how.exclusive || how.truncate) {
      await call('write-file', {
        path: target,
        data: '',
        exclusive: how.exclusive,
        mode,
      })
    } else {
      await call('write-file', { path: target, data: '', append: true, mode })
    }
  } else {
    // Opening for read must fail now if the file is missing.
    await call('stat', { path: target })
  }
  return new FileHandle(target, how)
}

export const promises = {
  constants,
  open,
  readFile: async (path: PathLike, options?: ReadOptions): Promise<any> => {
    const url = bundleUrl(toPath(path))
    return url === null
      ? decode(await call('read-file', { path: toPath(path) }), options)
      : withEncoding(await readBundled(url, toPath(path)), options)
  },
  writeFile: async (path: PathLike, data: unknown, options?: any) => {
    await call('write-file', writeArgs(path, data, options, false))
  },
  appendFile: async (path: PathLike, data: unknown, options?: any) => {
    await call('write-file', writeArgs(path, data, options, true))
  },
  stat: async (path: PathLike, options?: any) =>
    isBundled(path)
      ? bundledStats(await bundleEntry(toPath(path)), toPath(path), options)
      : new Stats(await call('stat', { path: toPath(path) }), statOptions(options)),
  lstat: async (path: PathLike, options?: any) =>
    isBundled(path)
      ? bundledStats(await bundleEntry(toPath(path)), toPath(path), options)
      : new Stats(await call('lstat', { path: toPath(path) }), statOptions(options)),
  access: async (path: PathLike, mode?: number) => {
    if (isBundled(path)) {
      bundledStats(await bundleEntry(toPath(path)), toPath(path), undefined)
      return
    }
    await call('access', { path: toPath(path), mode: mode ?? 0 })
  },
  readdir: async (path: PathLike, options?: any) =>
    direntResult(
      path,
      options,
      isBundled(path)
        ? bundledListing(await bundleEntry(toPath(path)), toPath(path))
        : await call('readdir', { path: toPath(path) })
    ),
  mkdir: async (path: PathLike, options?: any) => {
    await call('mkdir', mkdirArgs(path, options))
  },
  mkdtemp: (prefix: string): Promise<string> => call('mkdtemp', { prefix }),
  rm: async (path: PathLike, options?: any) => {
    await call('rm', rmArgs(path, options))
  },
  rmdir: async (path: PathLike) => {
    await call('rmdir', { path: toPath(path) })
  },
  unlink: async (path: PathLike) => {
    await call('unlink', { path: toPath(path) })
  },
  rename: async (from: PathLike, to: PathLike) => {
    await call('rename', { path: toPath(from), to: toPath(to) })
  },
  copyFile: async (from: PathLike, to: PathLike) => {
    await call('copy-file', { path: toPath(from), to: toPath(to) })
  },
  cp: async (from: PathLike, to: PathLike, options?: any) => {
    await call(options?.recursive === true ? 'copy' : 'copy-file', {
      path: toPath(from),
      to: toPath(to),
    })
  },
  realpath: async (path: PathLike): Promise<string> => {
    if (isBundled(path)) {
      // Bundled resources have no symlinks: a path that exists is canonical.
      bundledStats(await bundleEntry(toPath(path)), toPath(path), undefined)
      return toPath(path)
    }
    return call('realpath', { path: toPath(path) })
  },
  readlink: (path: PathLike): Promise<string> =>
    call('readlink', { path: toPath(path) }),
  symlink: async (target: PathLike, path: PathLike) => {
    await call('symlink', { path: toPath(path), target: toPath(target) })
  },
  chmod: async (path: PathLike, mode: number) => {
    await call('chmod', { path: toPath(path), mode })
  },
  truncate: async (path: PathLike) => {
    await call('write-file', writeArgs(path, new Uint8Array(0), undefined, false))
  },
  opendir: async (path: PathLike) => new Dir(toPath(path), await call('readdir', { path: toPath(path) })),
}

// --- synchronous ---------------------------------------------------------------

// A synchronous call carries its arguments in a URL (see `invokeSync`), which
// caps one call at about 64 KB. Larger writes go out as a first write that
// creates or truncates the file followed by appends.
const SyncWriteChunk = 16 * 1024

function writeSyncChunked(
  path: PathLike,
  data: unknown,
  options: any,
  append: boolean
) {
  const encoding = typeof options === 'string' ? options : options?.encoding
  const bytes = toBytes(data, encoding)
  let offset = 0
  do {
    const chunk = bytes.subarray(offset, offset + SyncWriteChunk)
    const first = offset === 0
    callSync('write-file', {
      ...writeArgs(path, chunk, first ? options : undefined, append || !first),
      // Only the first write may create the file exclusively.
      ...(first ? {} : { exclusive: false }),
    })
    offset += SyncWriteChunk
  } while (offset < bytes.byteLength)
}

export const readFileSync = (path: PathLike, options?: ReadOptions): any => {
  const url = bundleUrl(toPath(path))
  return url === null
    ? decode(callSync('read-file', { path: toPath(path) }), options)
    : withEncoding(readBundledSync(url, toPath(path)), options)
}
export const writeFileSync = (path: PathLike, data: unknown, options?: any) => {
  writeSyncChunked(path, data, options, false)
}
export const appendFileSync = (path: PathLike, data: unknown, options?: any) => {
  writeSyncChunked(path, data, options, true)
}
export const existsSync = (path: PathLike): boolean => {
  try {
    if (isBundled(path)) {
      return bundleEntrySync(toPath(path)) !== null
    }
    return callSync('exists', { path: toPath(path) }) === true
  } catch {
    return false
  }
}
export const statSync = (path: PathLike, options?: any) =>
  isBundled(path)
    ? bundledStats(bundleEntrySync(toPath(path)), toPath(path), options)
    : new Stats(callSync('stat', { path: toPath(path) }), statOptions(options))
export const lstatSync = (path: PathLike, options?: any) =>
  isBundled(path)
    ? bundledStats(bundleEntrySync(toPath(path)), toPath(path), options)
    : new Stats(callSync('lstat', { path: toPath(path) }), statOptions(options))
export const accessSync = (path: PathLike, mode?: number) => {
  if (isBundled(path)) {
    bundledStats(bundleEntrySync(toPath(path)), toPath(path), undefined)
    return
  }
  callSync('access', { path: toPath(path), mode: mode ?? 0 })
}
export const readdirSync = (path: PathLike, options?: any): any =>
  direntResult(
    path,
    options,
    isBundled(path)
      ? bundledListing(bundleEntrySync(toPath(path)), toPath(path))
      : callSync('readdir', { path: toPath(path) })
  )
export const mkdirSync = (path: PathLike, options?: any) => {
  callSync('mkdir', mkdirArgs(path, options))
}
export const mkdtempSync = (prefix: string): string =>
  callSync('mkdtemp', { prefix })
export const rmSync = (path: PathLike, options?: any) => {
  callSync('rm', rmArgs(path, options))
}
export const rmdirSync = (path: PathLike) => {
  callSync('rmdir', { path: toPath(path) })
}
export const unlinkSync = (path: PathLike) => {
  callSync('unlink', { path: toPath(path) })
}
export const renameSync = (from: PathLike, to: PathLike) => {
  callSync('rename', { path: toPath(from), to: toPath(to) })
}
export const copyFileSync = (from: PathLike, to: PathLike) => {
  callSync('copy-file', { path: toPath(from), to: toPath(to) })
}
export const chmodSync = (path: PathLike, mode: number) => {
  callSync('chmod', { path: toPath(path), mode })
}
export const readlinkSync = (path: PathLike): string =>
  callSync('readlink', { path: toPath(path) })
export const symlinkSync = (target: PathLike, path: PathLike) => {
  callSync('symlink', { path: toPath(path), target: toPath(target) })
}
function realpathSyncImpl(path: PathLike): string {
  if (isBundled(path)) {
    bundledStats(bundleEntrySync(toPath(path)), toPath(path), undefined)
    return toPath(path)
  }
  return callSync('realpath', { path: toPath(path) })
}
export const realpathSync = Object.assign(realpathSyncImpl, {
  native: realpathSyncImpl,
})

export class Dir {
  private index = 0
  public constructor(
    public readonly path: string,
    private readonly entries: IRawDirent[]
  ) {}
  public readSync() {
    const entry = this.entries[this.index++]
    return entry === undefined ? null : new Dirent(entry, this.path)
  }
  public async read() {
    return this.readSync()
  }
  public closeSync() {}
  public async close() {}
  public async *[Symbol.asyncIterator]() {
    for (const entry of this.entries) {
      yield new Dirent(entry, this.path)
    }
  }
}
export const opendirSync = (path: PathLike) =>
  new Dir(toPath(path), callSync('readdir', { path: toPath(path) }))

// Descriptors are emulated by path, like FileHandle above.
const descriptors = new Map<number, string>()
let nextDescriptor = 3

function descriptorPath(fd: number): string {
  const path = descriptors.get(fd)
  if (path === undefined) {
    throw Object.assign(new Error('EBADF: bad file descriptor'), { code: 'EBADF' })
  }
  return path
}

export function openSync(path: PathLike, flags?: string | number, mode?: number) {
  const target = toPath(path)
  const how = openMode(flags)
  if (how.create) {
    callSync('write-file', {
      path: target,
      data: '',
      exclusive: how.exclusive,
      append: !how.exclusive && !how.truncate,
      mode,
    })
  } else {
    callSync('stat', { path: target })
  }
  const fd = nextDescriptor++
  descriptors.set(fd, target)
  return fd
}
export const closeSync = (fd: number) => {
  descriptors.delete(fd)
}
export const fstatSync = (fd: number, options?: any) =>
  statSync(descriptorPath(fd), options)
export function readSync(
  fd: number,
  buffer: Uint8Array,
  offset = 0,
  length = buffer.byteLength - offset,
  position: number | null = 0
) {
  const bytes: Buffer = readFileSync(descriptorPath(fd))
  const start = position ?? 0
  const chunk = bytes.subarray(start, start + length)
  buffer.set(chunk, offset)
  return chunk.byteLength
}
export function writeSync(fd: number, data: unknown) {
  const bytes = toBytes(data)
  writeSyncChunked(descriptorPath(fd), bytes, undefined, true)
  return bytes.byteLength
}
export const fsyncSync = (_fd: number) => {}
export const fdatasyncSync = (_fd: number) => {}

// --- callbacks and streams -----------------------------------------------------

type Callback<T> = (error: Error | null, value?: T) => void

function callbackify<T>(promise: Promise<T>, args: unknown[]) {
  const callback = args[args.length - 1]
  if (typeof callback === 'function') {
    promise.then(
      value => (callback as Callback<T>)(null, value),
      error => (callback as Callback<T>)(error)
    )
  }
}

const optionsOf = (args: unknown[]) =>
  typeof args[0] === 'function' ? undefined : (args[0] as any)

export function readFile(path: PathLike, ...args: unknown[]) {
  callbackify(promises.readFile(path, optionsOf(args)), args)
}
export function writeFile(path: PathLike, data: unknown, ...args: unknown[]) {
  callbackify(promises.writeFile(path, data, optionsOf(args)), args)
}
export function stat(path: PathLike, ...args: unknown[]) {
  callbackify(promises.stat(path, optionsOf(args)), args)
}
export function lstat(path: PathLike, ...args: unknown[]) {
  callbackify(promises.lstat(path, optionsOf(args)), args)
}
export function readdir(path: PathLike, ...args: unknown[]) {
  callbackify(promises.readdir(path, optionsOf(args)), args)
}
export function mkdir(path: PathLike, ...args: unknown[]) {
  callbackify(promises.mkdir(path, optionsOf(args)), args)
}
export function unlink(path: PathLike, ...args: unknown[]) {
  callbackify(promises.unlink(path), args)
}
export function access(path: PathLike, ...args: unknown[]) {
  callbackify(promises.access(path, optionsOf(args)), args)
}
export function realpath(path: PathLike, ...args: unknown[]) {
  callbackify(promises.realpath(path), args)
}

type Listener = (...args: any[]) => void

/**
 * The part of a readable stream the renderer uses: `data`/`end`/`error`
 * events and `pipe`. The file is read in one piece.
 */
export class ReadStream {
  private readonly listeners = new Map<string, Listener[]>()
  private destroyed = false

  public constructor(path: PathLike, options?: any) {
    const encoding = encodingOf(options)
    promises.readFile(path).then(
      (bytes: Buffer) => {
        if (this.destroyed) {
          return
        }
        const start = options?.start ?? 0
        // Node's `end` is inclusive.
        const end = options?.end === undefined ? bytes.byteLength : options.end + 1
        const chunk = bytes.subarray(start, end)
        if (chunk.byteLength > 0) {
          this.emit('data', encoding ? chunk.toString(encoding) : chunk)
        }
        this.emit('end')
        this.emit('close')
      },
      error => this.emit('error', error)
    )
  }

  public on(event: string, listener: Listener) {
    const list = this.listeners.get(event) ?? []
    list.push(listener)
    this.listeners.set(event, list)
    return this
  }
  public once(event: string, listener: Listener) {
    const wrapper: Listener = (...args) => {
      this.off(event, wrapper)
      listener(...args)
    }
    return this.on(event, wrapper)
  }
  public off(event: string, listener: Listener) {
    const list = this.listeners.get(event) ?? []
    this.listeners.set(event, list.filter(l => l !== listener))
    return this
  }
  public removeListener(event: string, listener: Listener) {
    return this.off(event, listener)
  }
  public removeAllListeners() {
    this.listeners.clear()
    return this
  }
  public emit(event: string, ...args: any[]) {
    for (const listener of [...(this.listeners.get(event) ?? [])]) {
      listener(...args)
    }
    return true
  }
  public pipe<T extends { write(chunk: any): any; end(): any }>(destination: T): T {
    this.on('data', chunk => destination.write(chunk))
    this.on('end', () => destination.end())
    return destination
  }
  public destroy() {
    this.destroyed = true
    return this
  }
  public close() {
    this.destroyed = true
  }
}

export const createReadStream = (path: PathLike, options?: any) =>
  new ReadStream(path, options)

export class WriteStream {
  public constructor() {
    throw new Error('fs.createWriteStream is not available in the webview')
  }
}
export function createWriteStream(): WriteStream {
  return new WriteStream()
}

export default {
  constants, promises, Stats, BigIntStats, Dirent, Dir, ReadStream, WriteStream,
  readFileSync, writeFileSync, appendFileSync, existsSync, statSync, lstatSync,
  accessSync, readdirSync, mkdirSync, mkdtempSync, rmSync, rmdirSync, unlinkSync,
  renameSync, copyFileSync, chmodSync, readlinkSync, symlinkSync, realpathSync,
  opendirSync, openSync, closeSync, fstatSync, readSync, writeSync, fsyncSync,
  fdatasyncSync, readFile, writeFile, stat, lstat, readdir, mkdir, unlink,
  access, realpath, createReadStream, createWriteStream,
}
