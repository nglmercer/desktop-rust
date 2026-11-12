import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { formatJson, formatJsonFiles } from './format-json'

describe('release JSON formatting', () => {
  it('formats JSON with Biome and preserves values', () => {
    const input = '{"releases":{"v1":["one","two"]}}'
    const output = formatJson(input)
    expect(output).not.toBe(input)
    expect(output.endsWith('\n')).toBe(true)
    expect(JSON.parse(output)).toEqual(JSON.parse(input))
    expect(formatJson(output)).toBe(output)
  })

  it('leaves all files intact when any JSON is invalid', () => {
    const root = mkdtempSync(join(tmpdir(), 'release-json-'))
    try {
      const first = join(root, 'path with spaces.json')
      const second = join(root, 'invalid.json')
      writeFileSync(first, '{"valid":true}')
      writeFileSync(second, '{invalid')
      expect(() => formatJsonFiles([first, second])).toThrow()
      expect(readFileSync(first, 'utf8')).toBe('{"valid":true}')
      expect(readFileSync(second, 'utf8')).toBe('{invalid')
      writeFileSync(second, '{"other":[1,2]}')
      formatJsonFiles([first, second])
      expect(JSON.parse(readFileSync(first, 'utf8'))).toEqual({ valid: true })
      expect(JSON.parse(readFileSync(second, 'utf8'))).toEqual({
        other: [1, 2],
      })
      expect(readFileSync(first, 'utf8').endsWith('\n')).toBe(true)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
})
