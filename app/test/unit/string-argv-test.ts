import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { parseArgsStringToArgv } from '../../src/lib/string-argv'

describe('parseArgsStringToArgv', () => {
  it('preserves the legacy custom-integration argument parsing behavior', () => {
    assert.deepEqual(parseArgsStringToArgv(''), [])
    assert.deepEqual(parseArgsStringToArgv('a b  c'), ['a', 'b', 'c'])
    assert.deepEqual(parseArgsStringToArgv('"a b" c'), ['a b', 'c'])
    assert.deepEqual(parseArgsStringToArgv("'a b' c"), ['a b', 'c'])
    assert.deepEqual(parseArgsStringToArgv('abc"def ghi"jkl z'), [
      'abc"def ghi"jkl',
      'z',
    ])
    assert.deepEqual(parseArgsStringToArgv('--x=hello\\ world'), [
      '--x=hello\\',
      'world',
    ])
  })
})
