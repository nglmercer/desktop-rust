import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const calls = vi.hoisted(() => ({
  transport: 'ws',
  trailers: vi.fn(),
  config: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: calls.transport,
    repositoryInterpretTrailers: calls.trailers,
  }),
}))
vi.mock('../../src/lib/git/config', () => ({ getConfigValue: calls.config }))
vi.mock('../../src/lib/git/core', () => ({ git: calls.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: async (
    _path: string,
    _kind: string,
    operation: () => Promise<unknown>
  ) => operation(),
}))
import {
  mergeTrailers,
  parseTrailers,
} from '../../src/lib/git/interpret-trailers'

const repository = { path: '/repo' } as Repository
describe.each(['ws', 'wry'])('Rust trailer handling via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    calls.transport = transport
  })
  it('merges and parses using the repository configuration', async () => {
    calls.trailers
      .mockResolvedValueOnce('Summary\n\nReviewed-by: New\n')
      .mockResolvedValueOnce('Reviewed-by= New\n')
    calls.config.mockResolvedValue('=:')
    await expect(
      mergeTrailers(repository, 'Summary\n', [
        { token: 'Reviewed-by', value: 'New' },
      ])
    ).resolves.toContain('Reviewed-by: New')
    await expect(
      parseTrailers(repository, 'Summary\n\nReviewed-by= New\n')
    ).resolves.toEqual([{ token: 'Reviewed-by', value: 'New' }])
    expect(calls.trailers).toHaveBeenNthCalledWith(
      1,
      '/repo',
      'Summary\n',
      [{ token: 'Reviewed-by', value: 'New' }],
      false,
      false
    )
    expect(calls.trailers).toHaveBeenNthCalledWith(
      2,
      '/repo',
      'Summary\n\nReviewed-by= New\n',
      [],
      true,
      false
    )
    expect(calls.git).not.toHaveBeenCalled()
  })
  it('preserves the isolated environment fallback and propagates backend failures', async () => {
    calls.git.mockResolvedValue({ stdout: 'isolated' })
    await expect(
      mergeTrailers(repository, 'message', [], true, {
        env: { GIT_CONFIG_GLOBAL: '/isolated' },
      })
    ).resolves.toBe('isolated')
    expect(calls.git).toHaveBeenCalled()
    calls.trailers.mockRejectedValue(new Error('trailer command failed'))
    await expect(mergeTrailers(repository, 'message', [])).rejects.toThrow(
      'trailer command failed'
    )
  })
})
