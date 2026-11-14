// @vitest-environment node
import { mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { BackendClient } from '../../src/backend/client'
import {
  rustBackendAvailable,
  startRustBackend,
  type IRustBackend,
} from '../helpers/rust-backend'

const route = vi.hoisted(() => ({
  client: undefined as BackendClient | undefined,
}))
vi.mock('../../src/backend/client', async importOriginal => ({
  ...(await importOriginal<typeof import('../../src/backend/client')>()),
  backend: () => {
    if (!route.client) throw new Error('No Rust backend selected')
    return route.client
  },
}))

import { directoryExists } from '../../src/lib/directory-exists'
import { pathExists } from '../../src/lib/path-exists'

// The shared harness against the real bridge: `backend()` is routed to a live
// WebSocket client, so the existence helpers take their Rust branch and are
// answered by the server, not by Node's `fs`.
describe.skipIf(!rustBackendAvailable)('real Rust backend harness', () => {
  let rust: IRustBackend

  beforeEach(async () => {
    rust = await startRustBackend()
    route.client = rust.client
  })

  afterEach(async () => {
    route.client = undefined
    await rust.stop()
  })

  it('reports a ws transport so the Rust branches are taken', () => {
    expect(rust.client.transportKind).toBe('ws')
  })

  it('answers path existence from the Rust server', async () => {
    const file = path.join(rust.directory, 'file.txt')
    await writeFile(file, 'content')
    expect(await pathExists(file)).toBe(true)
    expect(await pathExists(path.join(rust.directory, 'missing'))).toBe(false)
    expect(await pathExists('')).toBe(false)
  })

  it('answers directory checks from the Rust server', async () => {
    const folder = path.join(rust.directory, 'folder')
    await mkdir(folder)
    await writeFile(path.join(rust.directory, 'plain'), 'x')
    expect(await directoryExists(folder)).toBe(true)
    expect(await directoryExists(path.join(rust.directory, 'plain'))).toBe(
      false
    )
    expect(await directoryExists(path.join(rust.directory, 'missing'))).toBe(
      false
    )
  })
})
