// @vitest-environment node
import assert from 'node:assert'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, it } from 'vitest'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'
import { readRegularFileVerifiedSync } from '../../src/web/fs'

describe('readRegularFileVerifiedSync', () => {
  let directory: string | undefined

  beforeEach(() => {
    installNodeHostBridge()
    directory = mkdtempSync(join(tmpdir(), 'desktop-verified-read-'))
  })

  afterEach(() => {
    uninstallNodeHostBridge()
    if (directory !== undefined) {
      rmSync(directory, { recursive: true, force: true })
      directory = undefined
    }
  })

  it('returns pinned bytes, canonical identity, and target metadata', () => {
    const file = join(directory!, 'config')
    writeFileSync(file, 'verified bytes')

    const result = readRegularFileVerifiedSync(file)

    assert.strictEqual(result.bytes.toString(), 'verified bytes')
    assert.strictEqual(result.matches, true)
    assert.strictEqual(result.stat.isFile(), true)
    assert.strictEqual(result.target?.isFile(), true)
    assert.ok(result.physical?.endsWith('config'))
  })
})
