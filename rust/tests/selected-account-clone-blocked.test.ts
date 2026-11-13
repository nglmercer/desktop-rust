// @vitest-environment node
import assert from 'node:assert/strict'
import { execFile, spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import { afterAll, beforeAll, describe, it } from 'vitest'

const prototype = path.resolve('rust/target/debug/examples/selected_clone_prototype')

describe.skipIf(process.env.DESKTOP_TEST_GIT_CREDS !== '1' || process.platform !== 'linux')('selected-account clone prototype rejects unsafe operations', () => {
  let root: string
  beforeAll(async () => {
    root = mkdtempSync(path.join(os.tmpdir(), 'desktop-blocked-clone-'))
    await promisify(execFile)('cargo', ['build', '-p', 'desktop-cli', '--example', 'selected_clone_prototype', '--features', 'clone-prototype', '--locked'], { cwd: 'rust', timeout: 300_000 })
  }, 360_000)
  afterAll(() => { if (root) rmSync(root, { recursive: true, force: true }) })

  it('rejects clone and recovery before touching the destination or registry', () => {
    const registry = path.join(root, 'registry.json')
    const destination = path.join(root, 'clone')
    const clone = spawnSync(prototype, [registry, 'https://github.com/example/repo.git', destination, 'github.com', 'alice', '1'], { encoding: 'utf8', timeout: 30_000 })
    assert.notEqual(clone.status, 0)
    assert.match(clone.stderr, /Selected-account cloning is temporarily unavailable/)
    assert.equal(existsSync(destination), false)
    assert.equal(existsSync(registry), false)
    const recovery = spawnSync(prototype, ['recover', registry, '11111111-1111-4111-8111-111111111111'], { encoding: 'utf8', timeout: 30_000 })
    assert.notEqual(recovery.status, 0)
    assert.match(recovery.stderr, /Selected-account clone recovery is temporarily unavailable/)
    assert.equal(existsSync(registry), false)
  })
})
