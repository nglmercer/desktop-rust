import * as React from 'react'
import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '../../helpers/ui/vitest-render'

const api = vi.hoisted(() => ({ history: vi.fn().mockResolvedValue([]) }))
vi.mock('../../../src/backend/client', () => ({ backend: () => ({ transportKind: 'ws', remoteBranchDeletionHistory: api.history }) }))
vi.mock('../../../src/ui/dialog', () => ({
  Dialog: ({ children, onSubmit }: { children: React.ReactNode; onSubmit: () => void }) => <form onSubmit={event => { event.preventDefault(); onSubmit() }}>{children}</form>,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogFooter: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))
vi.mock('../../../src/ui/dialog/ok-cancel-button-group', () => ({ OkCancelButtonGroup: ({ okButtonDisabled }: { okButtonDisabled: boolean }) => <button type="submit" disabled={okButtonDisabled}>Delete</button> }))
vi.mock('../../../src/ui/lib/ref', () => ({ Ref: ({ children }: { children: React.ReactNode }) => <span>{children}</span> }))

import { DeleteRemoteBranch } from '../../../src/ui/delete-branch/delete-remote-branch-dialog'
import { DeleteBranch } from '../../../src/ui/delete-branch/delete-branch-dialog'
import type { Dispatcher } from '../../../src/ui/dispatcher'
import type { Repository } from '../../../src/models/repository'
import type { Branch } from '../../../src/models/branch'

const repository = { path: '/repo' } as Repository
const branch = { name: 'origin/topic', upstreamRemoteName: 'origin', tip: { sha: 'b'.repeat(40) } } as Branch
const review = { basis: 'a'.repeat(64), remote: 'origin', reference: 'refs/heads/topic', endpoints: [{ index: 0, destination: '/first.git', displaySafe: true, expected: 'b'.repeat(40), availableLocally: true }, { index: 1, destination: '/second.git', displaySafe: true, expected: null, availableLocally: false }] }

describe('remote deletion dialogs', () => {
  it('does not delete until all push destinations have been reviewed', async () => {
    let finish!: (value: typeof review) => void
    const preview = vi.fn(() => new Promise<typeof review>(resolve => { finish = resolve }))
    const apply = vi.fn().mockResolvedValue({ receipt: '12345678-1234-4234-8234-123456789abc', warnings: ['Tip unavailable for recovery'] })
    const dispatcher = { previewRemoteBranchDeletion: preview, deleteReviewedRemoteBranch: apply } as unknown as Dispatcher
    render(<DeleteRemoteBranch repository={repository} branch={branch} dispatcher={dispatcher} onDismissed={() => {}} onDeleted={() => {}} />)
    expect((screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled).toBe(true)
    finish(review)
    await waitFor(() => expect(screen.getByText(/Destination 1:/)).toBeTruthy())
    expect(screen.getByText(/Destination 2:/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }))
    await waitFor(() => expect(apply).toHaveBeenCalledWith(repository, review))
    expect(screen.getByText(/Deletion receipt:/)).toBeTruthy()
    expect(screen.getByText('Tip unavailable for recovery')).toBeTruthy()
  })

  it('does not reuse a review when the upstream checkbox is switched off', async () => {
    let finish!: (value: typeof review) => void
    const preview = vi.fn(() => new Promise<typeof review>(resolve => { finish = resolve }))
    const dispatcher = { previewRemoteBranchDeletion: preview, deleteLocalBranch: vi.fn().mockResolvedValue(undefined) } as unknown as Dispatcher
    render(<DeleteBranch repository={repository} branch={branch} dispatcher={dispatcher} existsOnRemote={true} onDismissed={() => {}} onDeleted={() => {}} />)
    const checkbox = screen.getByLabelText('Yes, delete this branch on the remote')
    fireEvent.click(checkbox)
    fireEvent.click(checkbox)
    finish(review)
    await waitFor(() => expect((screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled).toBe(false))
    expect(screen.queryByText(/Destination 1:/)).toBeNull()
  })

  it('blocks deletion when a destination cannot be displayed safely', async () => {
    const hidden = { ...review, endpoints: [{ ...review.endpoints[0], destination: 'Push destination 1 (URL hidden)', displaySafe: false }] }
    const apply = vi.fn()
    const dispatcher = { previewRemoteBranchDeletion: vi.fn().mockResolvedValue(hidden), deleteReviewedRemoteBranch: apply } as unknown as Dispatcher
    render(<DeleteRemoteBranch repository={repository} branch={branch} dispatcher={dispatcher} onDismissed={() => {}} onDeleted={() => {}} />)
    await waitFor(() => expect(screen.getByText(/cannot be displayed safely/)).toBeTruthy())
    expect((screen.getByRole('button', { name: 'Delete' }) as HTMLButtonElement).disabled).toBe(true)
    expect(apply).not.toHaveBeenCalled()
  })

  it('shows recorded destination identities during recovery', async () => {
    api.history.mockResolvedValueOnce([{ id: '12345678-1234-4234-8234-123456789abc', reference: 'refs/heads/topic', phase: 'partial', endpoints: [
      { index: 0, destination: 'https://example.test/repo (credentials and query hidden)', displaySafe: true, phase: 'deleted', backupOid: 'b'.repeat(40) },
      { index: 1, destination: 'Push destination 2 (URL hidden)', displaySafe: false, phase: 'pending', backupOid: null },
    ] }])
    const dispatcher = { previewRemoteBranchDeletion: vi.fn().mockResolvedValue(review) } as unknown as Dispatcher
    render(<DeleteRemoteBranch repository={repository} branch={branch} dispatcher={dispatcher} onDismissed={() => {}} onDeleted={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: 'View remote deletion recovery' }))
    await waitFor(() => expect(screen.getByText(/https:\/\/example.test\/repo \(credentials and query hidden\) — deleted/)).toBeTruthy())
    expect(screen.getByText(/Push destination 2 \(URL hidden\) — pending/)).toBeTruthy()
  })
})
