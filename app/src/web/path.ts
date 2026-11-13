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
const reservedDeviceName = /^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])$/i

/** Length of the root portion: `C:\`, `C:`, `\\server\share\` or `\`. */
function rootLength(p: string): number {
  if (p.length === 0) {
    return 0
  }
  if (isSep(p[0])) {
    if (p.length > 1 && isSep(p[1])) {
      // UNC: \\server\share\
      const parts = /^[\\/]{2}([^\\/]+)[\\/]+([^\\/]+)[\\/]?/.exec(p)
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

function normalizeInternal(
  p: string,
  canonicalizeNamespacedDevice: boolean,
  protectRelativeDevices: boolean
): string {
  if (p.length === 0) {
    return '.'
  }
  const namespacedDevice = /^[\\/]{2}[.?][\\/]([^\\/:]+):([\s\S]*)$/.exec(p)
  if (
    canonicalizeNamespacedDevice &&
    namespacedDevice &&
    reservedDeviceName.test(namespacedDevice[1])
  ) {
    const rest = namespacedDevice[2]
    let tail = normalizeSegments(rest, true).join('\\')
    if (tail.length > 0 && isSep(rest[rest.length - 1])) tail += '\\'
    return `\\\\?\\${namespacedDevice[1]}:\\${tail}`
  }
  const colon = p.indexOf(':')
  const reservedDevice = colon > 0 && reservedDeviceName.test(p.slice(0, colon))
  if (protectRelativeDevices && reservedDevice) {
    const rest = p.slice(colon + 1)
    let tail = normalizeSegments(rest, false).join('\\') || '.'
    if (rest.length > 0 && isSep(rest[rest.length - 1])) tail += '\\'
    // Match Node's reserved-device handling for CVE-2025-27210.
    return `.\\${p.slice(0, colon + 1)}${tail}`
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
  if (
    protectRelativeDevices &&
    !absolute &&
    root.length === 0 &&
    colon !== -1 &&
    (/^[a-zA-Z]:/.test(tail) || /:(?:[\\/]|$)/.test(p))
  ) {
    // Keep a relative path from becoming drive-qualified after normalization.
    return `.\\${tail}`
  }
  return root + tail
}

function normalize(p: string): string {
  return normalizeInternal(p, true, true)
}

function isAbsolute(p: string): boolean {
  return splitRoot(p).absolute
}

function join(...parts: string[]): string {
  const nonempty = parts.filter(part => part.length > 0)
  let joined = nonempty.join('\\')
  // Node deliberately leaves reserved-device segments unnormalized to avoid
  // treating their following `..` as ordinary directory traversal.
  if (
    joined.split('\\').some(part => {
      const colon = part.indexOf(':')
      return colon > 0 && reservedDeviceName.test(part.slice(0, colon))
    })
  ) {
    const first = nonempty[0]
    const explicitUNC =
      first.length > 2 && isSep(first[0]) && isSep(first[1]) && !isSep(first[2])
    if (!explicitUNC) joined = joined.replace(/^[\\/]{2,}/, '\\')
    return joined.replace(/\//g, '\\')
  }
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
  const normalized = normalizeInternal(resolved, false, false)
  // Node never returns a trailing separator except for a bare root.
  return normalized.length > rootLength(normalized)
    ? normalized.replace(/\\+$/, '')
    : normalized
}

function dirname(p: string): string {
  return parse(p).dir || '.'
}

function basename(p: string, suffix?: string): string {
  const driveLength = /^[a-zA-Z]:/.test(p) ? 2 : 0
  if (suffix !== undefined && suffix.length > 0 && suffix.length <= p.length) {
    if (suffix === p) return ''
    let start = driveLength
    let end = -1
    let componentEnd = -1
    let remaining = suffix.length - 1
    for (let i = p.length - 1; i >= driveLength; i--) {
      if (isSep(p[i])) {
        if (componentEnd !== -1) {
          start = i + 1
          break
        }
      } else {
        if (componentEnd === -1) componentEnd = i + 1
        if (remaining >= 0) {
          if (p[i] === suffix[remaining]) {
            remaining--
            if (remaining === -1) end = i
          } else {
            remaining = -1
            end = componentEnd
          }
        }
      }
    }
    if (start === end) end = componentEnd
    else if (end === -1) end = p.length
    return p.slice(start, end)
  }
  // UNC roots are also valid basenames (the share), unlike drive roots.
  const rest = p.slice(driveLength).replace(/[\\/]+$/, '')
  const index = Math.max(rest.lastIndexOf('\\'), rest.lastIndexOf('/'))
  return rest.slice(index + 1)
}

function extensionOfBase(base: string): string {
  const index = base.lastIndexOf('.')
  return index <= 0 || base === '..' ? '' : base.slice(index)
}

function extname(p: string): string {
  return extensionOfBase(basename(p))
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
  let end = p.length
  while (end > root.length && isSep(p[end - 1])) {
    end--
  }
  let start = end
  while (start > root.length && !isSep(p[start - 1])) {
    start--
  }
  const base = p.slice(start, end)
  const ext = extensionOfBase(base)
  const dir = start > root.length ? p.slice(0, start - 1) : root
  return {
    root,
    dir,
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
