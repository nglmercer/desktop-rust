import assert from 'node:assert/strict'
import { homedir } from 'node:os'
import { describe, it } from 'vitest'
import { untildify } from '../../src/lib/untildify'

describe('untildify', () => {
  it('expands a leading home directory component', () => {
    assert.equal(untildify('~'), homedir())
    assert.equal(untildify('~/repository'), `${homedir()}/repository`)
    assert.equal(untildify('~\\repository'), `${homedir()}\\repository`)
  })

  it('leaves other paths unchanged', () => {
    assert.equal(untildify('~account/repository'), '~account/repository')
    assert.equal(untildify('/tmp/repository'), '/tmp/repository')
  })
})
