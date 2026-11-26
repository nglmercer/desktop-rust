// @vitest-environment node
import { mkdtempSync, rmSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from '../helpers/node-host-bridge'
import { readlink } from '../../src/web/fs-promises'

describe.skipIf(process.platform === 'win32')('web fs readlink', () => {
  let root = ''

  beforeEach(() => {
    installNodeHostBridge()
    root = mkdtempSync(join(tmpdir(), 'desktop-readlink-'))
  })

  afterEach(() => {
    uninstallNodeHostBridge()
    rmSync(root, { recursive: true, force: true })
    root = ''
  })

  it('preserves non-UTF8 symlink target bytes for buffer reads', async () => {
    const target = Buffer.from([0xff, 0xfe, 0x2f, 0x80])
    const link = join(root, 'link')
    symlinkSync(target, link)

    const bytes = await readlink(link, { encoding: 'buffer' })
    expect(Buffer.isBuffer(bytes)).toBe(true)
    expect(bytes.equals(target)).toBe(true)
    expect(typeof (await readlink(link))).toBe('string')
  })
})
