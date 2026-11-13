import { describe, it } from 'vitest'
import assert from 'node:assert'
import {
  encodePathAsUrl,
  isPathWithin,
  resolveWithin,
} from '../../src/lib/path'
import { resolve, basename, join, posix, win32 } from 'path'
import { promises } from 'fs'
import { tmpdir } from 'os'

const { rmdir, mkdtemp, mkdir, rm, symlink, unlink, writeFile } = promises

describe('path', () => {
  describe('encodePathAsUrl', () => {
    if (__WIN32__) {
      it('normalizes path separators on Windows', () => {
        const dirName =
          'C:/Users/shiftkey\\AppData\\Local\\GitHubDesktop\\app-1.0.4\\resources\\app'
        const uri = encodePathAsUrl(dirName, 'folder/file.html')
        assert.equal(
          uri.startsWith('file:///C:/Users/shiftkey/AppData/Local/'),
          true
        )
      })

      it('encodes spaces and hashes', () => {
        const dirName =
          'C:/Users/The Kong #2\\AppData\\Local\\GitHubDesktop\\app-1.0.4\\resources\\app'
        const uri = encodePathAsUrl(dirName, 'index.html')
        assert.equal(
          uri.startsWith('file:///C:/Users/The%20Kong%20%232/'),
          true
        )
      })
    }

    if (__DARWIN__ || __LINUX__) {
      it('encodes spaces and hashes', () => {
        const dirName =
          '/Users/The Kong #2/AppData/Local/GitHubDesktop/app-1.0.4/resources/app'
        const uri = encodePathAsUrl(dirName, 'index.html')
        assert(uri.startsWith('file:///Users/The%20Kong%20%232/'))
      })
    }
  })

  describe('resolveWithin', () => {
    const root = process.cwd()

    it('fails for paths outside of the root', async () => {
      assert((await resolveWithin(root, join('..'))) === null)
      assert((await resolveWithin(root, join('..', '..'))) === null)
    })

    it('succeeds for paths that traverse out, and then back into, the root', async () => {
      assert.equal(await resolveWithin(root, join('..', basename(root))), root)
    })

    it('fails for paths containing null bytes', async () => {
      assert((await resolveWithin(root, 'foo\0bar')) === null)
    })

    it('succeeds for absolute relative paths as long as they stay within the root', async () => {
      const parent = resolve(root, '..')
      assert.equal(await resolveWithin(parent, root), root)
    })

    it('accepts the root and descendants but rejects sibling prefixes', async () => {
      const directory = await mkdtemp(join(tmpdir(), 'path-containment-'))
      const root = join(directory, 'repo')
      const sibling = join(directory, 'repo-other')
      try {
        await mkdir(root)
        await mkdir(sibling)
        await writeFile(join(root, 'inside'), 'inside')
        await writeFile(join(sibling, 'outside'), 'outside')
        assert.equal(await resolveWithin(root, '.'), root)
        assert.equal(await resolveWithin(root, 'inside'), join(root, 'inside'))
        assert.equal(
          await resolveWithin(root, '..', 'repo-other', 'outside'),
          null
        )
        if (!__WIN32__) {
          await symlink(sibling, join(root, 'link'))
          assert.equal(await resolveWithin(root, 'link', 'outside'), null)
        }
      } finally {
        await rm(directory, { recursive: true, force: true })
      }
    })

    it('checks path boundaries and drive changes with POSIX and Windows rules', () => {
      assert.equal(isPathWithin('/repo', '/repo', posix), true)
      assert.equal(isPathWithin('/repo', '/repo/nested', posix), true)
      assert.equal(isPathWithin('/repo', '/repo-other', posix), false)
      assert.equal(isPathWithin('/', '/repo', posix), true)
      assert.equal(isPathWithin('C:\\Repo', 'c:\\repo\\nested', win32), true)
      assert.equal(isPathWithin('C:\\Repo', 'C:\\Repo-other', win32), false)
      assert.equal(isPathWithin('C:\\Repo', 'D:\\Repo', win32), false)
      assert.equal(
        isPathWithin('\\\\server\\share\\repo', '\\\\server\\share\\repo-other', win32),
        false
      )
    })

    if (!__WIN32__) {
      it('fails for paths that use a symlink to traverse outside of the root', async () => {
        const tempDir = await mkdtemp(join(tmpdir(), 'path-test'))
        const symlinkName = 'dangerzone'
        const symlinkPath = join(tempDir, symlinkName)

        try {
          await symlink(resolve(tempDir, '..', '..'), symlinkPath)
          assert((await resolveWithin(tempDir, symlinkName)) === null)
        } finally {
          await unlink(symlinkPath)
          await rmdir(tempDir)
        }
      })

      it('succeeds for paths that use a symlink to traverse outside of the root and then back again', async () => {
        const tempDir = await mkdtemp(join(tmpdir(), 'path-test'))
        const symlinkName = 'dangerzone'
        const symlinkPath = join(tempDir, symlinkName)

        try {
          await symlink(resolve(tempDir, '..', '..'), symlinkPath)
          const throughSymlinkPath = join(
            symlinkName,
            basename(resolve(tempDir, '..')),
            basename(tempDir)
          )
          assert.equal(
            await resolveWithin(tempDir, throughSymlinkPath),
            resolve(tempDir, throughSymlinkPath)
          )
        } finally {
          await unlink(symlinkPath)
          await rmdir(tempDir)
        }
      })
    }
  })
})
