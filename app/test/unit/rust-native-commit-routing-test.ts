import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({ prepare: vi.fn(), publish: vi.fn(), git: vi.fn() }))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({ transportKind: 'ws', prepareSelectedCommit: mocks.prepare, publishSelectedCommit: mocks.publish }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git, parseCommitSHA: vi.fn() }))
import { createCommit } from '../../src/lib/git/commit'

const repository = { path: '/repo' } as Repository
const review = {
  basis: 'a'.repeat(64), tree: 'b'.repeat(40), head: 'c'.repeat(40), branch: 'refs/heads/main',
  review: { threshold_bytes: 100, staged_count: 1, files: [], large_files: [] },
  selection: { paths: ['selected.txt'], remove_paths: [], partial_selections: [] },
}

afterEach(() => { vi.unstubAllGlobals(); vi.resetAllMocks() })

describe('ordinary native commit', () => {
  it('prepares the reviewed selection before publication and forwards progress and output', async () => {
    vi.stubGlobal('window', {})
    let finishPreparation!: (id: string) => void
    mocks.prepare.mockReturnValue(new Promise(resolve => { finishPreparation = resolve }))
    mocks.publish.mockImplementation(async (_path, _id, _message, options) => {
      options.onHookProgress({ hookName: 'pre-commit', status: 'started' })
      options.onOutput({ stream: 'stderr', bytes: new TextEncoder().encode('hook output') })
      options.onHookProgress({ hookName: 'pre-commit', status: 'finished' })
      return { oid: 'committed' }
    })
    const progress = vi.fn()
    const output = vi.fn()
    const result = createCommit(repository, 'message', [], {
      selectedReview: review, allowLargeFiles: true, onHookProgress: progress,
      onTerminalOutputAvailable: subscribe => { subscribe(output) },
    })
    expect(mocks.prepare).toHaveBeenCalledWith('/repo', review.selection, review.basis, expect.anything())
    expect(mocks.publish).not.toHaveBeenCalled()
    finishPreparation('checkpoint')
    await expect(result).resolves.toBe('committed')
    expect(mocks.publish).toHaveBeenCalledWith('/repo', 'checkpoint', 'message', expect.objectContaining({ allowLargeFiles: true }))
    expect(progress).toHaveBeenCalledWith(expect.objectContaining({ hookName: 'pre-commit', status: 'started', abort: expect.any(Function) }))
    expect(output.mock.calls[0][0].toString()).toBe('hook output')
    expect(mocks.git).not.toHaveBeenCalled()
  })
  it('leaves a failed hook checkpoint for inspection and never retries automatically', async () => {
    vi.stubGlobal('window', {})
    mocks.prepare.mockResolvedValue('checkpoint')
    mocks.publish.mockImplementation(async (_path, _id, _message, options) => {
      options.onHookProgress({ hookName: 'pre-commit', status: 'failed' })
      throw new Error('hook failed')
    })
    await expect(createCommit(repository, 'message', [], { selectedReview: review })).rejects.toThrow(/pre-commit hook failed.*checkpoint.*Ignoring an individual failed hook/)
    expect(mocks.publish).toHaveBeenCalledTimes(1)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
