import { win32 as nodeWin32 } from 'node:path'
import { describe, expect, it } from 'vitest'
import { win32 as browserWin32 } from '../../src/web/path'

describe('browser Windows path compatibility', () => {
  const paths = [
    '',
    '..',
    '...',
    '.gitignore',
    'C:',
    'C:\\',
    'C:\\repository\\file.txt',
    '\\\\server\\share',
    '\\\\server\\share\\',
    '\\\\server\\share.ext',
    '\\\\server\\share\\file.txt',
  ]

  for (const path of paths) {
    it(`matches Node basename, extension and parse for ${JSON.stringify(path)}`, () => {
      expect(browserWin32.basename(path)).toBe(nodeWin32.basename(path))
      expect(browserWin32.extname(path)).toBe(nodeWin32.extname(path))
      expect(browserWin32.parse(path)).toEqual(nodeWin32.parse(path))
    })
  }

  it('matches basename suffix handling', () => {
    for (const [path, suffix] of [
      ['file', 'file'],
      ['folder/file', 'file'],
      ['file.txt', '.txt'],
      ['\\\\server\\share', 'share'],
    ]) {
      expect(browserWin32.basename(path, suffix)).toBe(
        nodeWin32.basename(path, suffix)
      )
    }
  })

  it('retains UNC behavior in the shared root parser', () => {
    for (const path of [
      '\\\\server\\share\\\\',
      '\\\\server\\share////file',
      '//server/share//folder/file.txt',
    ]) {
      expect(browserWin32.normalize(path)).toBe(nodeWin32.normalize(path))
      expect(browserWin32.dirname(path)).toBe(nodeWin32.dirname(path))
      expect(browserWin32.resolve(path)).toBe(nodeWin32.resolve(path))
      expect(browserWin32.relative('\\\\server\\share\\base', path)).toBe(
        nodeWin32.relative('\\\\server\\share\\base', path)
      )
    }
  })

  it('matches Node across Windows roots, separators, dots and suffixes', () => {
    const roots = [
      '', 'C:', 'C:\\', 'D:/', '/', '\\',
      '\\\\server\\share', '\\\\server\\share\\', '//server/share/',
      '\\\\?\\C:\\',
    ]
    const parts = ['', 'a', '.', '..', '...', '.foo', 'a.', 'a..', 'file.ext']
    const separators = ['/', '\\', '//', '\\\\']
    for (const root of roots) {
      for (const left of parts) {
        for (const separator of separators) {
          for (const right of parts) {
            const path = root + left + separator + right
            expect(browserWin32.basename(path), path).toBe(nodeWin32.basename(path))
            expect(browserWin32.extname(path), path).toBe(nodeWin32.extname(path))
            expect(browserWin32.parse(path), path).toEqual(nodeWin32.parse(path))
            for (const suffix of ['', left, right, '.', 'a', 'file.ext']) {
              expect(browserWin32.basename(path, suffix), `${path} / ${suffix}`).toBe(
                nodeWin32.basename(path, suffix)
              )
            }
          }
        }
      }
    }
  })
})
