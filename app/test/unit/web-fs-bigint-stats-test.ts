import assert from 'node:assert'
import { lstatSync as nodeLstatSync } from 'node:fs'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, it } from 'vitest'
import { lstatSync } from '../../src/web/fs'
import { lstat } from '../../src/web/fs-promises'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'

describe('host-backed bigint filesystem stats', () => {
  beforeEach(() => installNodeHostBridge())
  afterEach(() => uninstallNodeHostBridge())

  it('preserves exact identity, ownership and nanosecond fields', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'desktop-web-bigint-stats-'))
    try {
      const path = join(directory, 'file')
      await writeFile(path, 'bytes')
      const native = nodeLstatSync(path, { bigint: true })
      for (const stat of [
        await lstat(path, { bigint: true }),
        lstatSync(path, { bigint: true }),
      ] as const) {
        assert.strictEqual(stat.dev, native.dev)
        assert.strictEqual(stat.ino, native.ino)
        assert.strictEqual(stat.mode, native.mode)
        assert.strictEqual(stat.nlink, native.nlink)
        assert.strictEqual(stat.size, native.size)
        assert.strictEqual(stat.mtimeNs, native.mtimeNs)
        assert.strictEqual(stat.ctimeNs, native.ctimeNs)
        assert.strictEqual(stat.isFile(), true)
        assert.strictEqual(stat.isDirectory(), false)
        assert.strictEqual(stat.isSymbolicLink(), false)
      }
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })
})
