import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DiffSelection, DiffSelectionType } from '../../src/models/diff'
import type { Repository } from '../../src/models/repository'
import {
  AppFileStatusKind,
  WorkingDirectoryFileChange,
  WorkingDirectoryStatus,
} from '../../src/models/status'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  read: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryLargeFiles: mocks.read,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
import { getLargeFilePaths } from '../../src/lib/large-files'

const repository = { path: '/repo' } as Repository
const file = (path: string, selected: boolean) =>
  new WorkingDirectoryFileChange(
    path,
    { kind: AppFileStatusKind.Modified },
    DiffSelection.fromInitialSelection(
      selected ? DiffSelectionType.All : DiffSelectionType.None
    )
  )

describe.each(['wry', 'ws'])('Rust file-size review via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })
  it('checks only selected files and returns large paths', async () => {
    mocks.read.mockResolvedValue([false, true])
    const status = WorkingDirectoryStatus.fromFiles([
      file('small', true),
      file('excluded', false),
      file('large', true),
    ])
    await expect(getLargeFilePaths(repository, status)).resolves.toEqual([
      'large',
    ])
    expect(mocks.read).toHaveBeenCalledWith('/repo', ['small', 'large'])
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
  })
  it('propagates review errors and never silently treats them as small', async () => {
    mocks.read.mockRejectedValue(new Error('unavailable'))
    const status = WorkingDirectoryStatus.fromFiles([file('large', true)])
    await expect(getLargeFilePaths(repository, status)).rejects.toThrow(
      'unavailable'
    )
  })
  it('splits long selections within the request bound', async () => {
    mocks.read.mockImplementation(async (_path, paths: string[]) =>
      paths.map(() => false)
    )
    const status = WorkingDirectoryStatus.fromFiles(
      Array.from({ length: 1100 }, (_, i) => file(`file-${i}`, true))
    )
    await expect(getLargeFilePaths(repository, status)).resolves.toEqual([])
    expect(mocks.read).toHaveBeenCalledTimes(2)
  })
})
