// @vitest-environment node
import { execFileSync } from 'node:child_process'
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import {
  collectRustLicenseInventory,
  writeRustLicenseInventory,
} from './rust-license-inventory'

const roots: string[] = []
afterEach(() => {
  for (const root of roots.splice(0))
    rmSync(root, { recursive: true, force: true })
})

function fixture() {
  const root = mkdtempSync(path.join(os.tmpdir(), 'desktop-rust-licenses-'))
  roots.push(root)
  for (const name of [
    'app',
    'cli',
    'normal',
    'build',
    'unused',
    'output/meta',
  ]) {
    mkdirSync(path.join(root, name), { recursive: true })
  }
  const make = (name: string, id = name) => ({
    id,
    name:
      name === 'app' ? 'desktop-app' : name === 'cli' ? 'desktop-cli' : name,
    version: '1.0.0',
    manifest_path: path.join(root, name, 'Cargo.toml'),
    source:
      name === 'normal'
        ? 'git+https://user:password@example.invalid/repo?token=secret#abcdef123456'
        : null,
    repository:
      name === 'normal'
        ? 'https://user:password@example.invalid/project?secret=token#deadbeef0123'
        : null,
    license: 'MIT',
    license_file: null as string | null,
  })
  const app = make('app'),
    cli = make('cli'),
    normal = make('normal'),
    build = make('build'),
    unused = make('unused')
  const dependency = (pkg: string, kind: string | null) => ({
    pkg,
    dep_kinds: [{ kind }],
  })
  const metadata = {
    packages: [app, cli, normal, build, unused],
    workspace_members: ['app', 'cli'],
    resolve: {
      nodes: [
        {
          id: 'app',
          deps: [dependency('normal', null), dependency('build', 'build')],
        },
        { id: 'cli', deps: [dependency('normal', null)] },
        { id: 'normal', deps: [] },
        { id: 'build', deps: [] },
        { id: 'unused', deps: [] },
      ],
    },
  }
  return { root, output: path.join(root, 'output'), metadata, normal }
}

describe('Rust package license inventory', () => {
  it('collects only reachable normal dependencies, preserves notices and identifies gaps', () => {
    const { root, output, metadata } = fixture()
    writeFileSync(path.join(root, 'normal/LICENSE-MIT'), 'Notice A\n')
    writeFileSync(path.join(root, 'normal/NOTICE'), 'Notice B\n')
    writeFileSync(path.join(root, 'normal/COPYRIGHT'), 'Copyright C\n')
    writeFileSync(path.join(root, 'normal/THIRD_PARTY.txt'), 'Third party D\n')
    writeFileSync(path.join(root, 'normal/AUTHORS'), 'Authors E\n')
    mkdirSync(path.join(root, 'normal/third_party'))
    mkdirSync(path.join(root, 'normal/LICENSES'))
    writeRustLicenseInventory(metadata, output, 'test-target')
    const inventory = JSON.parse(
      readFileSync(path.join(output, 'meta/rust-dependencies.json'), 'utf8')
    )
    expect(inventory.target).toBe('test-target')
    expect(inventory.packages.map((pkg: { name: string }) => pkg.name)).toEqual(
      ['desktop-app', 'desktop-cli', 'normal']
    )
    expect(inventory.packages[0].noLocalNoticeFound).toBe(true)
    const dependency = inventory.packages[2]
    expect(dependency.license).toBe('MIT')
    expect(dependency.source).toBe(
      'git+https://example.invalid/repo#abcdef123456'
    )
    expect(dependency.repository).toBe('https://example.invalid/project')
    expect(dependency.ignoredNoticeDirectories).toEqual([
      'LICENSES',
      'third_party',
    ])
    expect(JSON.stringify(inventory)).not.toMatch(/password|secret|token/)
    expect(
      dependency.notices.map((notice: { name: string }) => notice.name)
    ).toEqual([
      'AUTHORS',
      'COPYRIGHT',
      'LICENSE-MIT',
      'NOTICE',
      'THIRD_PARTY.txt',
    ])
    for (const notice of dependency.notices) {
      expect(
        readFileSync(path.join(output, 'licenses/rust', `${notice.sha256}.txt`))
      ).toEqual(readFileSync(path.join(root, 'normal', notice.name)))
    }
  })

  it('writes the same inventory regardless of metadata order or fixture location', () => {
    const first = fixture(),
      second = fixture()
    for (const item of [first, second])
      writeFileSync(path.join(item.root, 'normal/LICENSE'), 'The same notice\n')
    second.metadata.packages.reverse()
    second.metadata.resolve.nodes.reverse()
    second.metadata.resolve.nodes
      .find(node => node.id === 'app')
      ?.deps.reverse()
    writeRustLicenseInventory(first.metadata, first.output, 'target')
    writeRustLicenseInventory(second.metadata, second.output, 'target')
    expect(
      readFileSync(path.join(first.output, 'meta/rust-dependencies.json'))
    ).toEqual(
      readFileSync(path.join(second.output, 'meta/rust-dependencies.json'))
    )
  })

  it('reports a declared but absent license file', () => {
    const { output, metadata, normal } = fixture()
    normal.license_file = 'missing-license.txt'
    writeRustLicenseInventory(metadata, output, 'target')
    const inventory = JSON.parse(
      readFileSync(path.join(output, 'meta/rust-dependencies.json'), 'utf8')
    )
    expect(inventory.packages[2]).toMatchObject({
      missingDeclaredFile: true,
      noLocalNoticeFound: true,
    })
  })

  it('rejects declared paths outside a crate', () => {
    const { root, output, metadata, normal } = fixture()
    normal.license_file = path.join(root, 'outside-license.txt')
    writeFileSync(normal.license_file, 'private')
    expect(() => writeRustLicenseInventory(metadata, output, 'target')).toThrow(
      'escapes its package'
    )
  })

  it.skipIf(process.platform === 'win32')(
    'rejects license symlinks instead of copying an external file',
    () => {
      const { root, output, metadata } = fixture()
      const external = path.join(root, 'private')
      writeFileSync(external, 'private')
      symlinkSync(external, path.join(root, 'normal/LICENSE'))
      expect(() =>
        writeRustLicenseInventory(metadata, output, 'target')
      ).toThrow('symbolic link')
    }
  )

  it.skipIf(process.platform === 'win32')(
    'rejects a symlinked parent directory for a declared license',
    () => {
      const { root, output, metadata, normal } = fixture()
      const external = path.join(root, 'external')
      mkdirSync(external)
      writeFileSync(path.join(external, 'notice.txt'), 'private')
      symlinkSync(external, path.join(root, 'normal/link'), 'dir')
      normal.license_file = 'link/notice.txt'
      expect(() =>
        writeRustLicenseInventory(metadata, output, 'target')
      ).toThrow('symbolic link')
    }
  )

  it.skipIf(process.platform === 'win32')(
    'rejects dangling symlinks in declared license paths',
    () => {
      for (const parent of [false, true]) {
        const { root, output, metadata, normal } = fixture()
        const link = path.join(root, 'normal', parent ? 'link' : 'notice.txt')
        symlinkSync(
          path.join(root, 'does-not-exist'),
          link,
          parent ? 'dir' : 'file'
        )
        normal.license_file = parent ? 'link/notice.txt' : 'notice.txt'
        expect(() =>
          writeRustLicenseInventory(metadata, output, 'target')
        ).toThrow('symbolic link')
      }
    }
  )

  it('rejects incomplete dependency graphs', () => {
    const { output, metadata } = fixture()
    metadata.resolve.nodes = metadata.resolve.nodes.filter(
      node => node.id !== 'normal'
    )
    expect(() => writeRustLicenseInventory(metadata, output, 'target')).toThrow(
      'Incomplete Cargo dependency graph'
    )
  })

  it('rejects unknown dependency kinds instead of omitting them', () => {
    const { output, metadata } = fixture()
    metadata.resolve.nodes[0].deps[0].dep_kinds[0].kind = 'unknown'
    expect(() => writeRustLicenseInventory(metadata, output, 'target')).toThrow(
      'Invalid Cargo dependency'
    )
  })

  it('uses Cargo target filtering for normal dependencies', () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'desktop-cargo-targets-'))
    roots.push(root)
    for (const name of ['app', 'cli', 'linux-dep', 'windows-dep']) {
      mkdirSync(path.join(root, name, 'src'), { recursive: true })
      writeFileSync(path.join(root, name, 'src/lib.rs'), '')
    }
    writeFileSync(
      path.join(root, 'Cargo.toml'),
      '[workspace]\nresolver = "2"\nmembers = ["app", "cli", "linux-dep", "windows-dep"]\n'
    )
    const packageToml = (name: string) =>
      `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\nlicense = "MIT"\n`
    writeFileSync(
      path.join(root, 'app/Cargo.toml'),
      packageToml('desktop-app') +
        '[features]\npackaged-ui = []\n[target.\'cfg(target_os = "linux")\'.dependencies]\nlinux-dep = { path = "../linux-dep" }\n[target.\'cfg(target_os = "windows")\'.dependencies]\nwindows-dep = { path = "../windows-dep" }\n'
    )
    writeFileSync(
      path.join(root, 'cli/Cargo.toml'),
      packageToml('desktop-cli') +
        '[features]\nembedded-ui = []\npackaged-runtime = []\n'
    )
    for (const name of ['linux-dep', 'windows-dep'])
      writeFileSync(path.join(root, name, 'Cargo.toml'), packageToml(name))
    execFileSync('cargo', ['generate-lockfile', '--offline'], {
      cwd: root,
      stdio: 'pipe',
    })
    const lists = []
    for (const target of [
      'x86_64-unknown-linux-gnu',
      'x86_64-pc-windows-msvc',
    ]) {
      const output = path.join(root, target)
      mkdirSync(path.join(output, 'meta'), { recursive: true })
      collectRustLicenseInventory(root, target, output)
      lists.push(
        JSON.parse(
          readFileSync(path.join(output, 'meta/rust-dependencies.json'), 'utf8')
        ).packages.map((pkg: { name: string }) => pkg.name)
      )
    }
    expect(lists[0]).toContain('linux-dep')
    expect(lists[0]).not.toContain('windows-dep')
    expect(lists[1]).toContain('windows-dep')
    expect(lists[1]).not.toContain('linux-dep')
  }, 30_000)
})
