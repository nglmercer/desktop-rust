import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  constants,
  closeSync,
  existsSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  writeFileSync,
} from 'node:fs'
import * as path from 'node:path'

interface ICargoPackage {
  readonly id: string
  readonly name: string
  readonly version: string
  readonly manifest_path: string
  readonly source: string | null
  readonly repository: string | null
  readonly license: string | null
  readonly license_file: string | null
}

interface ICargoNode {
  readonly id: string
  readonly deps: ReadonlyArray<{
    readonly pkg: string
    readonly dep_kinds: ReadonlyArray<{ readonly kind: string | null }>
  }>
}

interface ICargoMetadata {
  readonly packages: ReadonlyArray<ICargoPackage>
  readonly workspace_members: ReadonlyArray<string>
  readonly resolve: { readonly nodes: ReadonlyArray<ICargoNode> }
}

/** Strip credentials and untrusted query data from a Cargo source or repository. */
function sourceUrl(
  value: string | null,
  preserveGitCommit = false
): string | null {
  if (!value) return null
  const match = /^(?:(registry|sparse|git)\+)?(.*)$/.exec(value)
  if (!match) return null
  try {
    const url = new URL(match[2])
    if (url.protocol !== 'https:' && url.protocol !== 'http:') return null
    url.username = ''
    url.password = ''
    url.search = ''
    if (
      !preserveGitCommit ||
      match[1] !== 'git' ||
      !/^#[0-9a-f]{7,64}$/i.test(url.hash)
    )
      url.hash = ''
    return `${match[1] ? `${match[1]}+` : ''}${url.toString()}`
  } catch {
    return null
  }
}

/** Check confinement and existing path components without following symlinks. */
function crateFileExists(root: string, file: string): boolean {
  const relative = path.relative(root, file)
  if (
    !relative ||
    relative === '..' ||
    relative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relative)
  ) {
    throw new Error('Cargo license file escapes its package directory')
  }
  let cursor = root
  for (const component of relative.split(path.sep)) {
    cursor = path.join(cursor, component)
    try {
      if (lstatSync(cursor).isSymbolicLink()) {
        throw new Error('Cargo license file contains a symbolic link')
      }
    } catch (error) {
      if (
        typeof error === 'object' &&
        error !== null &&
        'code' in error &&
        error.code === 'ENOENT'
      )
        return false
      throw error
    }
  }
  return true
}

/**
 * Read a bounded regular file after checking components for symlinks.
 * The pathname checks do not lock parent directories against concurrent changes.
 */
function readCrateFile(root: string, file: string): Buffer {
  if (!crateFileExists(root, file))
    throw new Error('Cargo license file disappeared while reading')
  const fd = openSync(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0))
  try {
    const info = fstatSync(fd)
    if (!info.isFile() || info.size > 2 * 1024 * 1024) {
      throw new Error('Cargo license file is not a bounded regular file')
    }
    const contents = Buffer.alloc(info.size + 1)
    let total = 0
    while (total < contents.length) {
      const count = readSync(fd, contents, total, contents.length - total, null)
      if (count === 0) break
      total += count
    }
    const after = fstatSync(fd)
    if (
      total !== info.size ||
      after.size !== info.size ||
      after.mtimeMs !== info.mtimeMs
    ) {
      throw new Error('Cargo license file changed while reading')
    }
    return contents.subarray(0, total)
  } finally {
    closeSync(fd)
  }
}

/** Write the resolved normal-dependency inventory and local notice texts. */
export function writeRustLicenseInventory(
  metadata: ICargoMetadata,
  destination: string,
  target: string
): void {
  if (
    !Array.isArray(metadata.packages) ||
    !Array.isArray(metadata.workspace_members) ||
    !metadata.workspace_members.every(id => typeof id === 'string') ||
    !Array.isArray(metadata.resolve?.nodes) ||
    typeof target !== 'string' ||
    target.length === 0
  ) {
    throw new Error('Invalid Cargo metadata')
  }
  const packages = new Map(metadata.packages.map(pkg => [pkg.id, pkg]))
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]))
  if (
    packages.size !== metadata.packages.length ||
    nodes.size !== metadata.resolve.nodes.length
  ) {
    throw new Error('Cargo metadata contains duplicate package identifiers')
  }
  const workspace = new Set(metadata.workspace_members)
  const roots = metadata.packages.filter(
    pkg =>
      workspace.has(pkg.id) &&
      (pkg.name === 'desktop-app' || pkg.name === 'desktop-cli')
  )
  if (roots.length !== 2 || new Set(roots.map(root => root.name)).size !== 2) {
    throw new Error('Cargo metadata is missing the desktop package roots')
  }
  const pending = roots.map(pkg => pkg.id)
  const selected = new Set<string>()
  while (pending.length > 0) {
    const id = pending.pop()
    if (!id || selected.has(id)) continue
    const node = nodes.get(id)
    if (!node || !packages.has(id) || !Array.isArray(node.deps))
      throw new Error('Incomplete Cargo dependency graph')
    selected.add(id)
    for (const dependency of node.deps) {
      if (
        !Array.isArray(dependency.dep_kinds) ||
        dependency.dep_kinds.length === 0 ||
        typeof dependency.pkg !== 'string' ||
        !dependency.dep_kinds.every(
          (kind: unknown) =>
            typeof kind === 'object' &&
            kind !== null &&
            'kind' in kind &&
            (kind.kind === null || kind.kind === 'build' || kind.kind === 'dev')
        )
      )
        throw new Error('Invalid Cargo dependency')
      if (
        dependency.dep_kinds.some(
          (kind: { kind: string | null }) => kind.kind === null
        )
      )
        pending.push(dependency.pkg)
    }
  }
  const licenseDirectory = path.join(destination, 'licenses', 'rust')
  mkdirSync(licenseDirectory, { recursive: true })
  const result = [...selected].map(id => {
    const pkg = packages.get(id)
    if (
      !pkg ||
      typeof pkg.id !== 'string' ||
      typeof pkg.name !== 'string' ||
      typeof pkg.version !== 'string' ||
      typeof pkg.manifest_path !== 'string' ||
      !(pkg.source === null || typeof pkg.source === 'string') ||
      !(pkg.repository === null || typeof pkg.repository === 'string') ||
      !(pkg.license === null || typeof pkg.license === 'string') ||
      !(pkg.license_file === null || typeof pkg.license_file === 'string')
    )
      throw new Error('Invalid Cargo package metadata')
    const root = realpathSync(path.dirname(pkg.manifest_path))
    const candidates = readdirSync(root, { withFileTypes: true }).filter(
      entry =>
        /^(?:licen[cs]es?|unlicense|copying|notice|copyright|authors|third[-_]party)(?:[._-]|$)/i.test(
          entry.name
        )
    )
    const ignoredNoticeDirectories = candidates
      .filter(entry => entry.isDirectory())
      .map(entry => entry.name)
      .sort()
    const files = candidates
      .filter(entry => !entry.isDirectory())
      .map(entry => path.join(root, entry.name))
    let missingDeclaredFile = false
    if (pkg.license_file) {
      const declared = path.resolve(root, pkg.license_file)
      if (!crateFileExists(root, declared)) missingDeclaredFile = true
      else files.push(declared)
    }
    const notices = [...new Set(files)].sort().map(file => {
      const content = readCrateFile(root, file)
      const digest = createHash('sha256').update(content).digest('hex')
      const stored = path.join(licenseDirectory, `${digest}.txt`)
      if (!existsSync(stored)) writeFileSync(stored, content, { flag: 'wx' })
      else if (!readFileSync(stored).equals(content))
        throw new Error('License content hash collision')
      return {
        name: path.relative(root, file).split(path.sep).join('/'),
        sha256: digest,
      }
    })
    return {
      name: pkg.name,
      version: pkg.version,
      license: pkg.license ?? null,
      repository: sourceUrl(pkg.repository),
      source: sourceUrl(pkg.source, true),
      local: pkg.source === null,
      notices,
      ignoredNoticeDirectories,
      missingDeclaredFile,
      noLocalNoticeFound: notices.length === 0,
    }
  })
  result.sort((a, b) => {
    const x = `${a.name}\0${a.version}\0${a.source ?? ''}`
    const y = `${b.name}\0${b.version}\0${b.source ?? ''}`
    if (x !== y) return x < y ? -1 : 1
    const left = JSON.stringify(a)
    const right = JSON.stringify(b)
    return left < right ? -1 : left > right ? 1 : 0
  })
  writeFileSync(
    path.join(destination, 'meta', 'rust-dependencies.json'),
    `${JSON.stringify(
      {
        scope:
          'Resolved normal Cargo dependencies reachable from the desktop and CLI workspace packages for the requested host. This can include extra workspace-unified features; it excludes build/dev dependencies, native system libraries, embedded code and npm dependencies. Only the explicitly declared license file and top-level LICENSE, LICENCE, LICENSES, LICENCES, UNLICENSE, COPYING, NOTICE, COPYRIGHT, AUTHORS and THIRD_PARTY files (including punctuation suffixes) are collected. Matching directories are recorded but not searched. This is not a complete license inventory.',
        target,
        packages: result,
      },
      null,
      2
    )}\n`
  )
}

/** Resolve locked Cargo metadata and fail packaging if it cannot be inspected. */
export function collectRustLicenseInventory(
  repo: string,
  host: string,
  destination: string
): void {
  const result = spawnSync(
    'cargo',
    [
      'metadata',
      '--locked',
      '--format-version=1',
      '--filter-platform',
      host,
      '--features',
      'desktop-cli/embedded-ui,desktop-cli/packaged-runtime,desktop-app/packaged-ui',
    ],
    { cwd: repo, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 }
  )
  if (result.error || result.status !== 0) {
    throw new Error(
      'Could not inspect locked Cargo metadata; check dependencies and Cargo network settings'
    )
  }
  let metadata: ICargoMetadata
  try {
    metadata = JSON.parse(result.stdout) as ICargoMetadata
  } catch {
    throw new Error('Cargo returned invalid metadata')
  }
  writeRustLicenseInventory(metadata, destination, host)
}
