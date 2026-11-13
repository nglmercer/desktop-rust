import { beforeEach, describe, expect, it, vi } from 'vitest'
import { FileChange, AppFileStatusKind } from '../../src/models/status'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws', snapshot: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, run: () => Promise<unknown>) => run()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, repositoryReadSnapshot: mocks.snapshot }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('keytar', () => ({ default: {} }))
vi.mock('../../src/lib/custom-integration', () => ({}))

import { getWorkingDirectoryImage } from '../../src/lib/git/diff'

describe.each(['wry', 'ws'])('working image over %s', transport => {
  beforeEach(() => { vi.clearAllMocks(); mocks.transport = transport })
  it('preserves the exact byte slice and normalizes the MIME extension', async () => {
    mocks.snapshot.mockResolvedValue(new Uint8Array([9, 0, 255, 9]).subarray(1, 3))
    const image = await getWorkingDirectoryImage({ path: '/repo' } as Repository, new FileChange('[a].PNG', { kind: AppFileStatusKind.Modified }))
    expect(Array.from(new Uint8Array(image.rawContents))).toEqual([0, 255])
    expect(image.contents).toBe('AP8=')
    expect(image.bytes).toBe(2)
    expect(image.mediaType).toBe('image/png')
    expect(mocks.snapshot).toHaveBeenCalledWith('/repo', 'renderer_working_file', { path: '[a].PNG' })
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
  })
  it('propagates missing-file failures', async () => {
    mocks.snapshot.mockRejectedValueOnce(new Error('missing'))
    await expect(getWorkingDirectoryImage({ path: '/repo' } as Repository, new FileChange('missing.png', { kind: AppFileStatusKind.Modified }))).rejects.toThrow('missing')
  })
})
