import { describe, expect, it } from 'vitest'
import { nativeFileDropPaths } from '../../src/lib/native-file-drop'

describe('native file drop events', () => {
  it('preserves the paths and order of a native drop', () => {
    const paths = ['/tmp/a b', '/tmp/ñ']
    expect(
      nativeFileDropPaths(
        new CustomEvent('desktop-native-file-drop', {
          detail: paths,
        })
      )
    ).toEqual(paths)
  })

  it.each([
    null,
    [],
    [''],
    ['/tmp/good', 1],
    ['/tmp/a\0b'],
    Array(257).fill('/tmp/repo'),
    [`/${'a'.repeat(64 * 1024)}`],
  ])('rejects invalid data without returning partial paths', detail => {
    expect(
      nativeFileDropPaths(
        new CustomEvent('desktop-native-file-drop', {
          detail,
        })
      )
    ).toBeNull()
  })

  it('rejects ordinary events without a payload', () => {
    expect(
      nativeFileDropPaths(new Event('desktop-native-file-drop'))
    ).toBeNull()
  })
})
