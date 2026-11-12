import { describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({ canonical: vi.fn(), realpath: vi.fn() }))
vi.mock('node:async_hooks', async () =>
  vi.importActual('../../src/web/async_hooks')
)
vi.mock('fs/promises', () => ({
  realpath: mocks.realpath,
  default: { realpath: mocks.realpath },
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: 'ws',
    canonicalMutationPath: mocks.canonical,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))

import { createRepositoryOperationContext } from '../../src/lib/git/repository-operation-context'
import {
  acquireAssistedCommitGitLease,
  requireRepositoryGitAsyncOwnership,
} from '../../src/lib/git/repository-operation'

describe('browser assisted transaction ownership', () => {
  it('identifies the shim as unable to propagate ownership across awaits', async () => {
    const context = createRepositoryOperationContext<string>()
    expect(context.supportsAsyncPropagation).toBe(false)
    await context.run('owner', async () => {
      expect(context.getStore()).toBe('owner')
      await Promise.resolve()
      expect(context.getStore()).toBeUndefined()
    })
  })

  it('rejects before acquiring a lease or inspecting repository paths', async () => {
    expect(() => requireRepositoryGitAsyncOwnership()).toThrow(
      'ownership cannot be maintained'
    )
    await expect(acquireAssistedCommitGitLease('/repo')).rejects.toMatchObject({
      code: 'unsupported-runtime',
    })
    expect(mocks.canonical).not.toHaveBeenCalled()
    expect(mocks.realpath).not.toHaveBeenCalled()
  })
})
