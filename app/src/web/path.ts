// `path` for the webview build. POSIX behaviour comes from path-browserify;
// that package ships no `win32` namespace, which the renderer uses explicitly
// (and which must be the default flavour on a Windows host).
import posixImpl from 'path-browserify'

interface IParsedPath {
  root: string
  dir: string
  base: string
  ext: string
  name: string
}

const isSep = (c: string) => c === '\\' || c === '/'

/** Length of the root portion: `C:\`, `C:`, `\\server\share\` or `\`. */
function rootLength(p: string): number {
  if (p.length === 0) {
    return 0
  }
  if (isSep(p[0])) {
    if (p.length > 1 && isSep(p[1])) {
      // UNC: \\server\share\
      const parts = /^[\\/]{2}([^\\/]+)[\\/]+([^\\/]+)([\\/]*)/.exec(p)
      if (parts !== null) {
        return parts[0].length
      }
    }
    return 1
  }
  if (/^[a-zA-Z]:/.test(p)) {
    return p.length > 2 && isSep(p[2]) ? 3 : 2
  }
  return 0
}

function splitRoot(p: string): {
  root: string
  rawRoot: string
  rest: string
  absolute: boolean
} {
  const length = rootLength(p)
  const rawRoot = p.slice(0, length)
  const absolute = length > 0 && (isSep(rawRoot[0]) || rawRoot.length > 2)
  let root = rawRoot.replace(/\//g, '\\')
  if (root.startsWith('\\\\')) {
    // Canonical UNC root: \\server\share\
    root = '\\\\' + root.split('\\').filter(Boolean).join('\\') + '\\'
  }
  return { root, rawRoot, rest: p.slice(length), absolute }
}

function normalizeSegments(rest: string, absolute: boolean): string[] {
  const out: string[] = []
  for (const segment of rest.split(/[\\/]+/)) {
    if (segment === '' || segment === '.') {
      continue
    }
    if (segment === '..') {
      if (out.length > 0 && out[out.length - 1] !== '..') {
        out.pop()
      } else if (!absolute) {
        out.push('..')
      }
      continue
    }
    out.push(segment)
  }
  return out
}

function normalize(p: string): string {
  if (p.length === 0) {
    return '.'
  }
  const { root, rest, absolute } = splitRoot(p)
  const trailing = rest.length > 0 && isSep(rest[rest.length - 1])
  let tail = normalizeSegments(rest, absolute).join('\\')
  if (tail.length === 0 && !absolute) {
    // `C:` is drive-relative, so it normalizes to `C:.` like Node.
    return root + (trailing ? '.\\' : '.')
  }
  if (tail.length > 0 && trailing) {
    tail += '\\'
  }
  return root + tail
}

function isAbsolute(p: string): boolean {
  return splitRoot(p).absolute
}

function join(...parts: string[]): string {
  const joined = parts.filter(part => part.length > 0).join('\\')
  return joined.length === 0 ? '.' : normalize(joined)
}

function resolve(...parts: string[]): string {
  let resolved = ''
  for (let i = parts.length - 1; i >= 0; i--) {
    const part = parts[i]
    if (part.length === 0) {
      continue
    }
    resolved = resolved.length === 0 ? part : `${part}\\${resolved}`
    if (isAbsolute(part)) {
      break
    }
  }
  const normalized = normalize(resolved)
  // Node never returns a trailing separator except for a bare root.
  return normalized.length > rootLength(normalized)
    ? normalized.replace(/\\+$/, '')
    : normalized
}

function dirname(p: string): string {
  const { rawRoot: root, rest } = splitRoot(p)
  const trimmed = rest.replace(/[\\/]+$/, '')
  const index = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'))
  if (index === -1) {
    return root.length > 0 ? root : '.'
  }
  return root + trimmed.slice(0, index).replace(/[\\/]+$/, '')
}

function basename(p: string, suffix?: string): string {
  const rest = splitRoot(p).rest.replace(/[\\/]+$/, '')
  const index = Math.max(rest.lastIndexOf('\\'), rest.lastIndexOf('/'))
  const base = rest.slice(index + 1)
  return suffix !== undefined && base !== suffix && base.endsWith(suffix)
    ? base.slice(0, -suffix.length)
    : base
}

function extname(p: string): string {
  const base = basename(p)
  const index = base.lastIndexOf('.')
  return index <= 0 ? '' : base.slice(index)
}

function relative(from: string, to: string): string {
  const a = resolve(from)
  const b = resolve(to)
  if (a.toLowerCase() === b.toLowerCase()) {
    return ''
  }
  const aRoot = splitRoot(a)
  const bRoot = splitRoot(b)
  if (aRoot.root.toLowerCase() !== bRoot.root.toLowerCase()) {
    return b
  }
  const aParts = aRoot.rest.split('\\').filter(Boolean)
  const bParts = bRoot.rest.split('\\').filter(Boolean)
  let common = 0
  while (
    common < aParts.length &&
    common < bParts.length &&
    aParts[common].toLowerCase() === bParts[common].toLowerCase()
  ) {
    common++
  }
  return [
    ...new Array(aParts.length - common).fill('..'),
    ...bParts.slice(common),
  ].join('\\')
}

function parse(p: string): IParsedPath {
  const { rawRoot: root } = splitRoot(p)
  const base = basename(p)
  const ext = extname(p)
  const dir = dirname(p)
  return {
    root,
    dir: dir === '.' && root.length === 0 ? '' : dir,
    base,
    ext,
    name: ext.length > 0 ? base.slice(0, -ext.length) : base,
  }
}

function format(parsed: Partial<IParsedPath>): string {
  const dir = parsed.dir || parsed.root || ''
  const base = parsed.base || `${parsed.name ?? ''}${parsed.ext ?? ''}`
  if (dir.length === 0) {
    return base
  }
  return dir === parsed.root ? dir + base : `${dir}\\${base}`
}

const win32Impl = {
  sep: '\\',
  delimiter: ';',
  normalize,
  isAbsolute,
  join,
  resolve,
  dirname,
  basename,
  extname,
  relative,
  parse,
  format,
  toNamespacedPath: (p: string) => p,
}

type PathModule = typeof posixImpl & {
  posix: PathModule
  win32: PathModule
}

export const posix = { ...posixImpl } as unknown as PathModule
export const win32 = { ...win32Impl } as unknown as PathModule
posix.posix = win32.posix = posix
posix.win32 = win32.win32 = win32

const native = __WIN32__ ? win32 : posix

export const sep = native.sep
export const delimiter = native.delimiter
const nativeNormalize = native.normalize
const nativeIsAbsolute = native.isAbsolute
const nativeJoin = native.join
const nativeResolve = native.resolve
const nativeDirname = native.dirname
const nativeBasename = native.basename
const nativeExtname = native.extname
const nativeRelative = native.relative
const nativeParse = native.parse
const nativeFormat = native.format
export {
  nativeNormalize as normalize,
  nativeIsAbsolute as isAbsolute,
  nativeJoin as join,
  nativeResolve as resolve,
  nativeDirname as dirname,
  nativeBasename as basename,
  nativeExtname as extname,
  nativeRelative as relative,
  nativeParse as parse,
  nativeFormat as format,
}

export default native
