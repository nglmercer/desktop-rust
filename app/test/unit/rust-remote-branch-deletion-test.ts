import { afterEach, describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

describe('reviewed remote branch deletion', () => {
  afterEach(() => Reflect.deleteProperty(window, '__desktop'))

  it('requires a reviewed destination list and carries its basis into deletion', async () => {
    const basis = 'a'.repeat(64)
    const tip = 'b'.repeat(40)
    const receipt = '12345678-1234-4234-8234-123456789abc'
    const invoke = vi.fn().mockResolvedValueOnce({ basis, remote: 'origin', reference: 'refs/heads/topic', endpoints: [
      { index: 0, destination: '/first.git', display_safe: true, expected: tip, available_locally: true },
      { index: 1, destination: '/second.git', display_safe: true, expected: null, available_locally: false },
    ] }).mockResolvedValueOnce({ code: 0, receipt_id: receipt, warnings: ['A tip is unavailable for recovery'] })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const review = await client.previewRemoteBranchDeletion('/repo', 'origin', 'topic')
    expect(review.endpoints).toEqual([{ index: 0, destination: '/first.git', displaySafe: true, expected: tip, availableLocally: true }, { index: 1, destination: '/second.git', displaySafe: true, expected: null, availableLocally: false }])
    expect(await client.applyRemoteBranchDeletion('/repo', review)).toEqual({ receipt, warnings: ['A tip is unavailable for recovery'] })
    expect(invoke.mock.calls[1][1].operation).toMatchObject({ type: 'remote_branch_deletion', operation: { method: 'delete_remote_branch', basis, request: { remote: 'origin', branch: 'topic', expected: null, fetch_backup: false } } })
  })

  it('rejects a stale local branch and treats partial remote failure as non-retryable', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ basis: 'a'.repeat(64), branch: { ref: 'refs/heads/topic', oid: 'b'.repeat(40) }, upstream: { basis: 'c'.repeat(64), remote: 'origin', reference: 'refs/heads/topic', endpoints: [{ index: 0, destination: '/first.git', display_safe: true, expected: 'b'.repeat(40), available_locally: true }] } })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    await expect(client.previewCombinedBranchDeletion('/repo', 'topic', 'd'.repeat(40), null, 'origin', 'topic')).rejects.toMatchObject({ code: 'branch_changed' })
    expect(invoke).toHaveBeenCalledTimes(1)
    invoke.mockResolvedValueOnce({ basis: 'a'.repeat(64), remote: 'origin', reference: 'refs/heads/topic', endpoints: [{ index: 0, destination: '/first.git', display_safe: true, expected: 'b'.repeat(40), available_locally: true }] }).mockRejectedValueOnce(new Error('destination 2 failed'))
    const review = await client.previewRemoteBranchDeletion('/repo', 'origin', 'topic')
    await expect(client.applyRemoteBranchDeletion('/repo', review)).rejects.toMatchObject({ retryable: false, message: expect.stringContaining('inspect') })
    expect(invoke).toHaveBeenCalledTimes(3)
  })

  it('does not discard valid recovery records when another record is unreadable', async () => {
    const invalid = '12345678-1234-4234-8234-123456789abc'
    const valid = '22345678-1234-4234-8234-123456789abc'
    const invoke = vi.fn().mockResolvedValue([{ id: invalid, error: 'Cannot read this receipt' }, { id: valid, reference: 'refs/heads/topic', phase: 'complete', endpoints: [{ index: 0, destination: 'https://example.test/repo (credentials and query hidden)', display_safe: true, phase: 'deleted', backup_oid: 'b'.repeat(40) }, { index: 1, destination: 'Push destination 2 (URL hidden)', display_safe: false, phase: 'pending', backup_oid: null }] }])
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const records = await new BackendClient().remoteBranchDeletionHistory('/repo')
    expect(records).toEqual([{ id: invalid, reference: 'Unreadable recovery record', phase: 'unreadable', endpoints: [] }, { id: valid, reference: 'refs/heads/topic', phase: 'complete', endpoints: [{ index: 0, destination: 'https://example.test/repo (credentials and query hidden)', displaySafe: true, phase: 'deleted', backupOid: 'b'.repeat(40) }, { index: 1, destination: 'Push destination 2 (URL hidden)', displaySafe: false, phase: 'pending', backupOid: null }] }])
  })

  it('does not delete when any destination URL is hidden', async () => {
    const invoke = vi.fn().mockResolvedValue({ basis: 'a'.repeat(64), remote: 'origin', reference: 'refs/heads/topic', endpoints: [{ index: 0, destination: 'Push destination 1 (URL hidden)', display_safe: false, expected: 'b'.repeat(40), available_locally: true }] })
    Object.defineProperty(window, '__desktop', { configurable: true, value: { invoke, send: vi.fn(), on: vi.fn() } })
    const client = new BackendClient()
    const review = await client.previewRemoteBranchDeletion('/repo', 'origin', 'topic')
    await expect(client.applyRemoteBranchDeletion('/repo', review)).rejects.toMatchObject({ code: 'invalid_request' })
    expect(invoke).toHaveBeenCalledTimes(1)
  })
})
