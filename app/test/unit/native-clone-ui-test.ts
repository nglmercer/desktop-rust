import { beforeEach, describe, expect, it, vi } from 'vitest'

const request = vi.hoisted(() => vi.fn())
const route = vi.hoisted(() => ({ kind: 'ws' }))
const reservations = vi.hoisted(() => ({ active: [] as string[] }))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ request, transportKind: route.kind }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  canonicalMutationPath: async (path: string) => `/canonical${path}`,
  withRepositoryGitResourceMutation: async (
    paths: string[],
    operation: () => Promise<unknown>
  ) => {
    reservations.active.push(...paths)
    try {
      return await operation()
    } finally {
      reservations.active.splice(-paths.length)
    }
  },
}))

import {
  listNativeCloneRecoveries,
  nativeClone,
  nativeCloneAccountHost,
  nativeCloneProgress,
  recoverNativeClone,
} from '../../src/ui/clone-repository/native-clone'

describe('native clone UI requests', () => {
  beforeEach(() => {
    request.mockReset()
    route.kind = 'ws'
  })

  it('preserves the selected Enterprise account port', () => {
    expect(nativeCloneAccountHost('https://github.example:8443/api/v3')).toBe(
      'github.example:8443'
    )
  })

  it('uses only the selected account and explicitly disables recursion', async () => {
    request.mockResolvedValue({
      kind: 'clone_completed_unselected',
      record: { path: '/canonical/repo' },
      retained_staging: '/canonical/.desktop-clone-x',
    })
    const account = { host: 'github.com', login: 'alice', id: 12 }
    expect(
      await nativeClone(
        'https://github.com/a/b',
        '/alias/repo',
        account,
        'main',
        () => {}
      )
    ).toEqual({
      path: '/canonical/repo',
      staging: '/canonical/.desktop-clone-x',
    })
    expect(request.mock.calls[0][1]).toMatchObject({
      account,
      recursive: false,
      report_progress: true,
      report_retained_staging: true,
    })
  })

  it('routes Wry clones and recovery through the typed host', async () => {
    route.kind = 'wry'
    const id = '11111111-1111-1111-1111-111111111111'
    const account = { host: 'github.com', login: 'alice', id: 12 }
    request
      .mockResolvedValueOnce({ kind: 'clone_completed_unselected', record: { path: '/repo' }, retained_staging: '/stage' })
      .mockResolvedValueOnce([{ id, destination: '/repo', stage: '/stage', recoverable: true }])
      .mockResolvedValueOnce({ path: '/repo', retained_staging: '/stage' })
    await nativeClone('https://github.com/a/b', '/repo', account, 'main', () => {})
    expect(request.mock.calls[0][0]).toBe('git_info')
    expect(request.mock.calls[0][1]).toEqual({ operation: { type: 'clone_selected', url: 'https://github.com/a/b', path: '/repo', account, default_branch: 'main' } })
    expect(request.mock.calls[0][2].deadlineMs).toBe(3_600_000)
    expect(await listNativeCloneRecoveries()).toHaveLength(1)
    expect(request.mock.calls[1]).toEqual(['git_info', { operation: { type: 'list_clone_recoveries' } }])
    await recoverNativeClone(id, '/repo', '/stage')
    expect(request.mock.calls[2][0]).toBe('git_info')
    expect(request.mock.calls[2][1]).toEqual({ operation: { type: 'recover_clone', recovery_id: id } })
  })

  it('shows only fixed progress categories', () => {
    expect(
      nativeCloneProgress({
        stream: 'stderr',
        data: btoa('Receiving objects: 42% (42/100)\r'),
      })
    ).toBe('Receiving objects: 42% (42/100)')
    expect(
      nativeCloneProgress({ stream: 'stderr', data: btoa('secret token') })
    ).toBeNull()
  })

  it('only offers recovery for records marked recoverable', async () => {
    request.mockResolvedValue([
      {
        id: '11111111-1111-1111-1111-111111111111',
        destination: '/tmp/repo',
        stage: '/tmp/stage',
        recoverable: false,
      },
    ])
    expect(await listNativeCloneRecoveries()).toEqual([
      {
        id: '11111111-1111-1111-1111-111111111111',
        destination: '/tmp/repo',
        recoverable: false,
        mutationPath: '/tmp/repo',
        stagePath: '/tmp/stage',
      },
    ])
    expect(request.mock.calls[0][1]).toEqual({ report_recovery_status: true })
  })

  it('preserves recovered path and reports uncertain responses', async () => {
    request
      .mockResolvedValueOnce({ path: '/repo', retained_staging: '/stage' })
      .mockResolvedValueOnce({})
    expect(
      await recoverNativeClone(
        '11111111-1111-1111-1111-111111111111',
        '/repo',
        '/stage'
      )
    ).toEqual({ path: '/repo', staging: '/stage' })
    await expect(
      recoverNativeClone(
        '11111111-1111-1111-1111-111111111111',
        '/repo',
        '/stage'
      )
    ).rejects.toThrow('inspect the destination')
  })

  it('displays non-UTF-8 paths without losing bytes and requires inspection', async () => {
    const id = '11111111-1111-1111-1111-111111111111'
    request
      .mockResolvedValueOnce([
        { id, destination: { bytes: [47, 255] }, recoverable: true },
      ])
      .mockResolvedValueOnce({
        path: { bytes: [47, 255] },
        retained_staging: { bytes: [47, 254] },
      })
    expect(await listNativeCloneRecoveries()).toEqual([
      {
        id,
        destination: 'Bytes: \\x2f\\xff',
        recoverable: false,
        mutationPath: null,
        stagePath: null,
      },
    ])
    expect(await recoverNativeClone(id, '/repo', '/stage')).toEqual({
      path: 'Bytes: \\x2f\\xff',
      staging: 'Bytes: \\x2f\\xfe',
    })
  })

  it('holds original and canonical resource reservations until clone finishes', async () => {
    let finish: (value: unknown) => void = () => {}
    request.mockImplementation(
      () =>
        new Promise(resolve => {
          finish = resolve
        })
    )
    const result = nativeClone(
      'https://github.com/a/b',
      '/repo',
      { host: 'github.com', login: 'alice', id: 12 },
      undefined,
      () => {}
    )
    await vi.waitFor(() =>
      expect(reservations.active).toEqual(['/repo', '/canonical/repo'])
    )
    finish({
      kind: 'clone_completed_unselected',
      record: { path: '/repo' },
      retained_staging: '/stage',
    })
    await result
    expect(reservations.active).toEqual([])
  })
})
