import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  backendStatus: vi.fn(),
  context: vi.fn(),
  conflicts: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, action: () => Promise<unknown>) =>
      action()
  ),
}))

vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    detailedRepositoryStatus: mocks.backendStatus,
    repositoryStatusContext: mocks.context,
    repositoryConflictDetails: mocks.conflicts,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/merge', () => ({
  isMergeHeadSet: async () => false,
  isSquashMsgSet: async () => false,
}))
vi.mock('../../src/lib/git/rebase', () => ({
  getRebaseInternalState: async () => null,
}))
vi.mock('../../src/lib/git/cherry-pick', () => ({
  isCherryPickHeadFound: async () => false,
}))
vi.mock('../../src/lib/git/diff-check', () => ({
  getFilesWithConflictMarkers: async () => new Map(),
}))
vi.mock('../../src/lib/git/diff', () => ({ getBinaryPaths: async () => [] }))

import { getStatus } from '../../src/lib/git/status'
import type { Repository } from '../../src/models/repository'

const bytes = (value: string) => ({ bytes: [...Buffer.from(value)] })
const repository = { path: '/repo-a' } as Repository

describe('Rust status integration', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
    mocks.context.mockResolvedValue({
      mergeHeadFound: false,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: null,
    })
  })

  it('uses bridge status and operation markers without a shell read', async () => {
    mocks.transport = 'ws'
    mocks.backendStatus.mockResolvedValue([
      { kind: 'header', value: bytes('branch.head main') },
      {
        kind: 'entry',
        entry_kind: '?',
        status_code: '??',
        submodule_status_code: '????',
        path: bytes('new-file'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    mocks.context.mockResolvedValue({
      mergeHeadFound: false,
      squashMsgFound: true,
      isCherryPickingHeadFound: true,
      rebaseInternalState: null,
    })
    const status = await getStatus(repository)
    expect(status?.currentBranch).toBe('main')
    expect(status?.squashMsgFound).toBe(true)
    expect(status?.isCherryPickingHeadFound).toBe(true)
    expect(status?.workingDirectory.files[0].path).toBe('new-file')
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses typed bridge conflict details without a shell read', async () => {
    mocks.transport = 'ws'
    mocks.backendStatus.mockResolvedValue([
      {
        kind: 'entry',
        entry_kind: 'u',
        status_code: 'UU',
        submodule_status_code: 'N...',
        path: bytes('file'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    mocks.context.mockResolvedValue({
      mergeHeadFound: true,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: null,
    })
    mocks.conflicts.mockResolvedValue({
      check:
        'file:1: leftover conflict marker\nfile:3: leftover conflict marker\n',
      numstat: '',
      attributes: 'file\0merge\0text\0',
    })
    const status = await getStatus(repository)
    expect(status?.workingDirectory.files[0].status).toMatchObject({
      conflictMarkerCount: 2,
    })
    expect(mocks.conflicts).toHaveBeenCalledWith('/repo-a', 'merge')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves binary merge-driver classification over the bridge', async () => {
    mocks.transport = 'ws'
    mocks.backendStatus.mockResolvedValue([
      {
        kind: 'entry',
        entry_kind: 'u',
        status_code: 'UU',
        submodule_status_code: 'N...',
        path: bytes('binary.dat'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    mocks.context.mockResolvedValue({
      mergeHeadFound: true,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: null,
    })
    mocks.conflicts.mockResolvedValue({
      check: '',
      numstat: '',
      attributes: 'binary.dat\0merge\0binary\0',
    })
    const status = await getStatus(repository)
    expect(status?.workingDirectory.files[0].status).not.toHaveProperty(
      'conflictMarkerCount'
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses Rust conflict details under the native repository lease', async () => {
    mocks.backendStatus.mockResolvedValue([
      {
        kind: 'entry',
        entry_kind: 'u',
        status_code: 'UU',
        submodule_status_code: 'N...',
        path: bytes('file'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    mocks.context.mockResolvedValue({
      mergeHeadFound: true,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: null,
    })
    mocks.conflicts.mockResolvedValue({
      check: 'file:1: leftover conflict marker\n',
      numstat: '',
      attributes: '',
    })
    const status = await getStatus(repository)
    expect(status?.workingDirectory.files[0].status).toMatchObject({
      conflictMarkerCount: 1,
    })
    expect(mocks.conflicts).toHaveBeenCalledWith('/repo-a', 'merge')
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('loads the selected repository under the read lease and maps the status', async () => {
    mocks.backendStatus.mockResolvedValue([
      { kind: 'header', value: bytes('branch.head main') },
      { kind: 'header', value: bytes('branch.oid abc123') },
      {
        kind: 'entry',
        entry_kind: '2',
        status_code: 'R.',
        submodule_status_code: 'N...',
        path: bytes('new\nname'),
        old_path: bytes('old name'),
        rename_or_copy_score: 90,
      },
      {
        kind: 'entry',
        entry_kind: '?',
        status_code: '??',
        submodule_status_code: '????',
        path: bytes('untracked'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    const status = await getStatus(repository, false)
    expect(mocks.backendStatus).toHaveBeenCalledWith('/repo-a', false)
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
    expect(status?.currentBranch).toBe('main')
    expect(status?.currentTip).toBe('abc123')
    expect(status?.workingDirectory.files.map(file => file.path)).toEqual([
      'new\nname',
      'untracked',
    ])
    expect(status?.workingDirectory.files[0].status).toMatchObject({
      oldPath: 'old name',
      renameIncludesModifications: true,
    })
  })

  it('uses the existing Git path when the bounded Rust read is unavailable', async () => {
    mocks.backendStatus.mockRejectedValue(new Error('status output too large'))
    mocks.git.mockResolvedValue({
      stdout: Buffer.from('# branch.head main\0? file\0'),
      exitCode: 0,
    })
    const status = await getStatus(repository)
    expect(mocks.git).toHaveBeenCalledOnce()
    expect(status?.workingDirectory.files[0].path).toBe('file')
  })

  it('uses Rust for strict status reads required during repository creation', async () => {
    mocks.backendStatus.mockResolvedValueOnce([
      { kind: 'header', value: bytes('branch.head main') },
      {
        kind: 'entry',
        entry_kind: '?',
        status_code: '??',
        submodule_status_code: '????',
        path: bytes('README.md'),
        old_path: null,
        rename_or_copy_score: null,
      },
    ])
    const status = await getStatus(repository, true, true)
    expect(status.workingDirectory.files[0].path).toBe('README.md')
    expect(mocks.backendStatus).toHaveBeenCalledWith('/repo-a', true)
    expect(mocks.context).toHaveBeenCalledWith('/repo-a')
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses explicitly scoped reads for an unselected repository during creation', async () => {
    mocks.transport = 'ws'
    mocks.backendStatus.mockResolvedValueOnce([])
    await getStatus(repository, true, true, true)
    expect(mocks.backendStatus).toHaveBeenCalledWith('/repo-a', true, {}, true)
    expect(mocks.context).toHaveBeenCalledWith('/repo-a', true)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates strict Rust status and context failures without a legacy retry', async () => {
    const failure = new Error('missing repository')
    mocks.backendStatus.mockRejectedValueOnce(failure)
    await expect(getStatus(repository, true, true)).rejects.toBe(failure)
    mocks.backendStatus.mockResolvedValueOnce([])
    mocks.context.mockRejectedValueOnce(failure)
    await expect(getStatus(repository, true, true)).rejects.toBe(failure)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates strict conflict-detail failures without a legacy retry', async () => {
    const failure = new Error('conflict details unavailable')
    mocks.backendStatus.mockResolvedValueOnce([])
    mocks.context.mockResolvedValueOnce({
      mergeHeadFound: true,
      squashMsgFound: false,
      isCherryPickingHeadFound: false,
      rebaseInternalState: null,
    })
    mocks.conflicts.mockRejectedValueOnce(failure)
    await expect(getStatus(repository, true, true)).rejects.toBe(failure)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates repository admission failures without retrying', async () => {
    const failure = new Error('repository ownership fence')
    mocks.scope.mockRejectedValueOnce(failure)
    await expect(getStatus(repository)).rejects.toBe(failure)
    expect(mocks.backendStatus).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
