import * as React from 'react'
import { backend, type IRemoteBranchDeletionReview } from '../../backend/client'
import type { Repository } from '../../models/repository'
import type { Dispatcher } from '../dispatcher'
import { withRepositoryGitOperation } from '../../lib/git/repository-operation'

/** Display the exact tips that will be deleted from each push destination. */
export function RemoteDeletionReview({
  review,
}: {
  readonly review: IRemoteBranchDeletionReview
}) {
  return (
    <div>
      <p>
        Review {review.reference} on {review.remote}. Each destination is
        checked again before deletion.
      </p>
      {review.local && (
        <p>
          Local branch {review.local.name} ({review.local.tip}) will also be
          deleted
          {review.local.checkout
            ? ` after switching to ${review.local.checkout}`
            : ''}
          .
        </p>
      )}
      <ol>
        {review.endpoints.map(endpoint => (
          <li key={endpoint.index}>
            Destination {endpoint.index + 1}: {endpoint.destination} —{' '}
            {endpoint.expected ?? 'branch absent'}
            {endpoint.expected !== null && !endpoint.availableLocally
              ? ' (tip is not available locally for recovery)'
              : ''}
          </li>
        ))}
      </ol>
      {review.endpoints.some(endpoint => !endpoint.displaySafe) && (
        <p role="alert">
          A destination URL cannot be displayed safely. Update the Git remote
          configuration before deleting this branch.
        </p>
      )}
    </div>
  )
}

type History = Awaited<
  ReturnType<ReturnType<typeof backend>['remoteBranchDeletionHistory']>
>
interface IRecoveryState {
  readonly open: boolean
  readonly loading: boolean
  readonly history: History
  readonly error: string | null
  readonly name: string
}

/** Inspect partial deletion receipts and restore a retained tip as a local branch. */
export class RemoteDeletionRecovery extends React.Component<
  { readonly repository: Repository; readonly dispatcher: Dispatcher },
  IRecoveryState
> {
  private mounted = false
  public constructor(props: {
    readonly repository: Repository
    readonly dispatcher: Dispatcher
  }) {
    super(props)
    this.state = {
      open: false,
      loading: false,
      history: [],
      error: null,
      name: '',
    }
  }

  public componentDidMount() {
    this.mounted = true
  }
  public componentWillUnmount() {
    this.mounted = false
  }

  private refresh = async () => {
    this.setState({ open: true, loading: true, error: null })
    try {
      const history = await backend().remoteBranchDeletionHistory(
        this.props.repository.path
      )
      if (this.mounted) this.setState({ history, loading: false })
    } catch {
      if (this.mounted)
        this.setState({
          loading: false,
          error:
            'Could not read deletion history. Inspect the repository before retrying.',
        })
    }
  }

  private restore = async (id: string, index: number) => {
    this.setState({ loading: true, error: null })
    try {
      await withRepositoryGitOperation(
        this.props.repository.path,
        'mutation',
        () =>
          backend().restoreRemoteBranchTip(
            this.props.repository.path,
            id,
            index,
            this.state.name
          ),
        true
      )
      await this.props.dispatcher.refreshRepository(this.props.repository)
      if (this.mounted) await this.refresh()
    } catch {
      if (this.mounted)
        this.setState({
          loading: false,
          error:
            'Recovery did not complete. Inspect local branches and deletion history before retrying.',
        })
    }
  }

  public render() {
    return (
      <div>
        <button
          type="button"
          onClick={this.refresh}
          disabled={this.state.loading}
        >
          View remote deletion recovery
        </button>
        {this.state.open && (
          <div>
            {this.state.error && <p role="alert">{this.state.error}</p>}
            {this.state.loading && <p>Loading deletion history…</p>}
            {!this.state.loading && this.state.history.length === 0 && (
              <p>No remote deletion receipts.</p>
            )}
            <label>
              New local branch name for recovery{' '}
              <input
                value={this.state.name}
                onChange={event =>
                  this.setState({ name: event.currentTarget.value })
                }
              />
            </label>
            {this.state.history.map(record => (
              <div key={record.id}>
                <p>
                  {record.reference} — {record.phase} ({record.id})
                </p>
                <ul>
                  {record.endpoints.map(endpoint => (
                    <li key={endpoint.index}>
                      Destination {endpoint.index + 1}: {endpoint.destination} —{' '}
                      {endpoint.phase}{' '}
                      {endpoint.backupOid ?? '(no retained tip)'}
                      {endpoint.backupOid !== null && (
                        <button
                          type="button"
                          disabled={
                            this.state.loading ||
                            this.state.name.trim().length === 0
                          }
                          onClick={() =>
                            this.restore(record.id, endpoint.index)
                          }
                        >
                          Restore as local branch
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )}
      </div>
    )
  }
}
