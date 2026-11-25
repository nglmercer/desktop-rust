import { describe, it } from 'vitest'
import assert from 'node:assert'
import { getTempFilePath } from '../../src/lib/file-system'
import { tmpdir } from 'os'

describe('file-system', () => {
  describe('getTempFilePath', () => {
    it('returns a path in the temp directory', () => {
      const result = getTempFilePath('test-file')
      assert.ok(result.startsWith(tmpdir()))
    })

    it('includes the given name in the path', () => {
      const result = getTempFilePath('my-temp-file')
      assert.ok(result.includes('my-temp-file'))
    })

    it('generates unique paths on each call', () => {
      const a = getTempFilePath('test')
      const b = getTempFilePath('test')
      assert.notEqual(a, b)
    })
  })
})
