import { describe, expect, it } from 'vitest'
import { backend } from '../../src/backend/client'
import { setupEmptyRepository } from '../helpers/repositories'
import { rustBackendAvailable, useRustBackend } from '../helpers/rust-backend'

describe.skipIf(!rustBackendAvailable)('Rust-backed repository fixture', () => {
  useRustBackend()

  it('initializes a reusable repository through the real backend', async t => {
    const repository = await setupEmptyRepository(t, 'main')
    expect(backend().transportKind).toBe('ws')
    await expect(
      backend().repositoryType(repository.path)
    ).resolves.toMatchObject({
      kind: 'regular',
      topLevelWorkingDirectory: repository.path,
    })
  })
})
