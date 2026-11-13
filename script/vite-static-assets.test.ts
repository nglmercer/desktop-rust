// @vitest-environment node
import assert from 'node:assert/strict'
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'vitest'
import { build } from 'vite'
import {
  copyGitignoreTemplates,
  copyStaticResources,
  moveSourceMaps,
} from '../vite.config'

describe('Vite gitignore template copying', () => {
  it.skipIf(process.platform === 'win32')(
    'preserves executable permissions of the macOS launcher across builds',
    async () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-launcher-mode-'))
      try {
        writeFileSync(
          path.join(root, 'index.html'),
          '<script type="module" src="/entry.js"></script>'
        )
        writeFileSync(path.join(root, 'entry.js'), 'console.log("fixture")')
        const output = path.join(root, 'output')
        const source = path.resolve('app/static/darwin/github.sh')
        const target = path.join(output, 'static/github.sh')
        const expected = lstatSync(source).mode & 0o777
        assert.notEqual(
          expected & 0o111,
          0,
          'macOS launcher must be executable'
        )
        const config = {
          configFile: false as const,
          root,
          logLevel: 'silent' as const,
          plugins: [copyStaticResources('darwin')],
          build: { outDir: output, emptyOutDir: false, minify: false as const },
        }
        await build(config)
        assert.equal(lstatSync(target).mode & 0o777, expected)
        assert.deepEqual(readFileSync(target), readFileSync(source))
        chmodSync(target, 0o600)
        await build(config)
        assert.equal(lstatSync(target).mode & 0o777, expected)
        assert.deepEqual(readFileSync(target), readFileSync(source))
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    },
    60_000
  )

  it('writes resources and source maps into the configured output, leaving the default untouched', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-custom-output-'))
    const defaultTemplate = path.resolve(
      'out/vite-dist/static/gitignore/Clojure.gitignore'
    )
    const before = existsSync(defaultTemplate)
      ? lstatSync(defaultTemplate)
      : null
    const previous = process.env.DESKTOP_VITE_SOURCEMAP
    try {
      writeFileSync(
        path.join(root, 'index.html'),
        '<script type="module" src="/entry.js"></script>'
      )
      writeFileSync(path.join(root, 'entry.js'), 'console.log("fixture")')
      process.env.DESKTOP_VITE_SOURCEMAP = '1'
      const output = path.join(root, 'custom')
      await build({
        configFile: false,
        root,
        logLevel: 'silent',
        plugins: [copyStaticResources(), moveSourceMaps()],
        build: { outDir: output, sourcemap: true, minify: false },
      })
      assert.ok(existsSync(path.join(output, 'emoji.json')))
      assert.ok(existsSync(path.join(output, 'emoji/shipit.png')))
      assert.ok(
        lstatSync(
          path.join(output, 'static/gitignore/Clojure.gitignore')
        ).isFile()
      )
      assert.ok(
        readdirSync(path.join(root, 'vite-sourcemaps/assets')).some(name =>
          name.endsWith('.map')
        )
      )
      const after = existsSync(defaultTemplate)
        ? lstatSync(defaultTemplate)
        : null
      assert.deepEqual(
        after === null ? null : [after.ino, after.mtimeMs, after.ctimeMs],
        before === null ? null : [before.ino, before.mtimeMs, before.ctimeMs]
      )
    } finally {
      if (previous === undefined) delete process.env.DESKTOP_VITE_SOURCEMAP
      else process.env.DESKTOP_VITE_SOURCEMAP = previous
      rmSync(root, { recursive: true, force: true })
    }
  }, 60_000)

  it('copies ordinary templates repeatedly', () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-gitignore-regular-'))
    try {
      const source = path.join(root, 'source')
      const destination = path.join(root, 'output')
      mkdirSync(source)
      writeFileSync(path.join(source, 'Plain.gitignore'), 'first\n')
      copyGitignoreTemplates(source, destination)
      writeFileSync(path.join(source, 'Plain.gitignore'), 'second\n')
      copyGitignoreTemplates(source, destination)
      assert.equal(
        readFileSync(path.join(destination, 'Plain.gitignore'), 'utf8'),
        'second\n'
      )
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  it.skipIf(process.platform === 'win32')(
    'replaces stale symlinks safely and creates portable files across repeated builds',
    () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-gitignore-copy-'))
      try {
        const source = path.join(root, 'source')
        const destination = path.join(root, 'output')
        const relocated = path.join(root, 'moved')
        const victim = path.join(root, 'victim')
        mkdirSync(source)
        mkdirSync(destination)
        writeFileSync(path.join(source, 'Actual.gitignore'), 'expected\n')
        symlinkSync('Actual.gitignore', path.join(source, 'Alias.gitignore'))
        writeFileSync(victim, 'keep\n')
        symlinkSync(victim, path.join(destination, 'Alias.gitignore'))
        copyGitignoreTemplates(source, destination)
        copyGitignoreTemplates(source, destination)
        assert.equal(readFileSync(victim, 'utf8'), 'keep\n')
        assert.equal(
          readFileSync(path.join(source, 'Actual.gitignore'), 'utf8'),
          'expected\n'
        )
        assert.ok(lstatSync(path.join(destination, 'Alias.gitignore')).isFile())
        renameSync(destination, relocated)
        rmSync(source, { recursive: true, force: true })
        assert.equal(
          readFileSync(path.join(relocated, 'Alias.gitignore'), 'utf8'),
          'expected\n'
        )
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }
  )

  it.skipIf(process.platform === 'win32')(
    'refuses template links outside the source directory',
    () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-gitignore-escape-'))
      try {
        const source = path.join(root, 'source')
        mkdirSync(source)
        writeFileSync(path.join(root, 'outside'), 'secret')
        symlinkSync(
          path.join(root, 'outside'),
          path.join(source, 'escape.gitignore')
        )
        assert.throws(
          () => copyGitignoreTemplates(source, path.join(root, 'output')),
          /escapes its source/
        )
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }
  )

  it.skipIf(process.platform === 'win32')(
    'refuses linked output directories and ancestors without altering their targets',
    () => {
      const root = mkdtempSync(
        path.join(tmpdir(), 'desktop-gitignore-output-link-')
      )
      try {
        const source = path.join(root, 'source')
        const outside = path.join(root, 'outside')
        mkdirSync(source)
        mkdirSync(outside)
        writeFileSync(path.join(source, 'Actual.gitignore'), 'source\n')
        writeFileSync(path.join(outside, 'Actual.gitignore'), 'keep\n')
        symlinkSync(outside, path.join(root, 'link'))
        assert.throws(
          () => copyGitignoreTemplates(source, path.join(root, 'link'), root),
          /Unsafe output directory/
        )
        assert.throws(
          () =>
            copyGitignoreTemplates(
              source,
              path.join(root, 'link', 'nested'),
              root
            ),
          /Unsafe output directory/
        )
        assert.equal(
          readFileSync(path.join(outside, 'Actual.gitignore'), 'utf8'),
          'keep\n'
        )
        assert.equal(
          readFileSync(path.join(source, 'Actual.gitignore'), 'utf8'),
          'source\n'
        )
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }
  )

  it('rejects overlapping source and output before creating directories', () => {
    const root = mkdtempSync(path.join(tmpdir(), 'desktop-gitignore-overlap-'))
    try {
      const source = path.join(root, 'source')
      mkdirSync(source)
      assert.throws(
        () => copyGitignoreTemplates(source, source, root),
        /overlap/
      )
      assert.throws(
        () => copyGitignoreTemplates(source, path.join(source, 'nested'), root),
        /overlap/
      )
      assert.throws(() => lstatSync(path.join(source, 'nested')), /ENOENT/)
      assert.throws(() => copyGitignoreTemplates(source, root, root), /overlap/)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  it.skipIf(process.platform === 'win32')(
    'does not follow existing resource links during a custom-output build',
    async () => {
      const root = mkdtempSync(path.join(tmpdir(), 'desktop-resource-links-'))
      try {
        const output = path.join(root, 'output')
        const external = path.join(root, 'external')
        const victim = path.join(external, 'victim')
        mkdirSync(output)
        mkdirSync(external)
        writeFileSync(victim, 'keep\n')
        writeFileSync(
          path.join(root, 'index.html'),
          '<script type="module" src="/entry.js"></script>'
        )
        writeFileSync(path.join(root, 'entry.js'), 'console.log("fixture")')
        mkdirSync(path.join(output, 'emoji'))
        symlinkSync(victim, path.join(output, 'emoji.json'))
        symlinkSync(victim, path.join(output, 'emoji/shipit.png'))
        const config = {
          configFile: false as const,
          root,
          logLevel: 'silent' as const,
          plugins: [copyStaticResources()],
          build: { outDir: output, emptyOutDir: false, minify: false as const },
        }
        await build(config)
        assert.equal(readFileSync(victim, 'utf8'), 'keep\n')
        assert.ok(lstatSync(path.join(output, 'emoji/shipit.png')).isFile())
        assert.ok(lstatSync(path.join(output, 'emoji.json')).isFile())
        rmSync(path.join(output, 'emoji'), { recursive: true })
        symlinkSync(external, path.join(output, 'emoji'))
        await assert.rejects(build(config), /Unsafe output directory/)
        assert.equal(readFileSync(victim, 'utf8'), 'keep\n')
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    },
    60_000
  )
})
