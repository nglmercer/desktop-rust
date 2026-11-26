import assert from 'node:assert'
import { lstatSync, renameSync, writeFileSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, it } from 'vitest'
import { constants } from '../../src/web/fs'
import { open } from '../../src/web/fs-promises'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'

describe('host-backed file handles', () => {
  beforeEach(() => installNodeHostBridge())
  afterEach(() => uninstallNodeHostBridge())

  it('pins the original inode for stat and reads across path replacement', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'desktop-web-handle-'))
    try {
      const path = join(directory, 'input')
      writeFileSync(path, 'original bytes')
      const original = lstatSync(path, { bigint: true })
      const handle = await open(path, constants.O_RDONLY | constants.O_NONBLOCK)
      try {
        const replacement = join(directory, 'replacement')
        writeFileSync(replacement, 'replacement bytes')
        renameSync(replacement, path)
        const held = await handle.stat({ bigint: true })
        assert.strictEqual(held.dev, original.dev)
        assert.strictEqual(held.ino, original.ino)
        assert.strictEqual(await handle.readFile('utf8'), 'original bytes')
        assert.strictEqual(
          lstatSync(path, { bigint: true }).ino === original.ino,
          false
        )
      } finally {
        await handle.close()
      }
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  it('writes through an exclusively created inode even after replacement', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'desktop-web-lock-handle-'))
    try {
      const path = join(directory, 'lock')
      const handle = await open(path, 'wx', 0o600)
      const original = lstatSync(path, { bigint: true })
      try {
        writeFileSync(join(directory, 'replacement'), 'foreign')
        renameSync(join(directory, 'replacement'), path)
        await handle.writeFile('owned bytes')
        await handle.sync()
        const held = await handle.stat({ bigint: true })
        assert.strictEqual(held.dev, original.dev)
        assert.strictEqual(held.ino, original.ino)
        assert.strictEqual(held.size, BigInt('owned bytes'.length))
        assert.strictEqual(
          lstatSync(path, { bigint: true }).ino === original.ino,
          false
        )
      } finally {
        await handle.close()
      }
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })
})
