import * as React from 'react'
import { backend, type IDiscardRecoveryRecord } from '../../backend/client'
import type { Repository } from '../../models/repository'
import type { Dispatcher } from '../dispatcher'
import { DefaultDialogFooter, Dialog, DialogContent } from '../dialog'
import { Button } from '../lib/button'

interface IProps {
  readonly repository: Repository
  readonly dispatcher: Dispatcher
  readonly disabled: boolean
}

interface IState {
  readonly open: boolean
  readonly loading: boolean
  readonly busy: boolean
  readonly records: ReadonlyArray<IDiscardRecoveryRecord>
  readonly error: string | null
  readonly confirming: string | null
}

/** Durable recovery remains available after the immediate Undo banner closes. */
export class DiscardRecovery extends React.Component<IProps, IState> {
  private generation = 0
  private mounted = false
  private operationInFlight = false
  public state: IState = {
    open: false,
    loading: false,
    busy: false,
    records: [],
    error: null,
    confirming: null,
  }

  public componentDidMount() {
    this.mounted = true
  }
  public componentWillUnmount() {
    this.mounted = false
    this.generation++
  }

  private open = () => {
    this.setState({ open: true }, () => {
      void this.load()
    })
  }

  private close = () => {
    if (this.operationInFlight) return
    this.generation++
    this.setState({
      open: false,
      loading: false,
      error: null,
      confirming: null,
    })
  }

  private async load() {
    const generation = ++this.generation
    this.setState({ loading: true, error: null })
    try {
      const records = await backend().repositoryDiscardHistory(
        this.props.repository.path
      )
      if (this.mounted && generation === this.generation)
        this.setState({ records, loading: false })
    } catch (error) {
      if (this.mounted && generation === this.generation)
        this.setState({
          loading: false,
          records: [],
          error:
            error instanceof Error
              ? error.message
              : 'Could not load recovery history.',
        })
    }
  }

  private restore = async (id: string) => {
    if (this.operationInFlight) return
    this.operationInFlight = true
    this.setState({ busy: true, error: null })
    try {
      await this.props.dispatcher.undoRepositoryDiscard(
        this.props.repository,
        id
      )
      if (this.mounted) {
        this.setState({ busy: false })
        await this.load()
      }
    } catch (error) {
      if (this.mounted)
        this.setState({
          busy: false,
          error:
            error instanceof Error
              ? error.message
              : 'Could not restore this entry.',
        })
    } finally {
      this.operationInFlight = false
    }
  }

  private forget = async (id: string) => {
    const record = this.state.records.find(entry => entry.id === id)
    if (
      this.operationInFlight ||
      this.state.confirming !== id ||
      !record ||
      (record.state !== 'applied' && record.state !== 'undone')
    )
      return
    this.operationInFlight = true
    this.setState({ busy: true, error: null })
    try {
      await this.props.dispatcher.forgetRepositoryDiscard(
        this.props.repository,
        id
      )
      if (this.mounted) {
        this.setState({ busy: false, confirming: null })
        await this.load()
      }
    } catch (error) {
      if (this.mounted)
        this.setState({
          busy: false,
          error:
            error instanceof Error
              ? error.message
              : 'Could not delete this backup.',
        })
    } finally {
      this.operationInFlight = false
    }
  }

  public render() {
    const { open, loading, busy, records, error, confirming } = this.state
    return (
      <>
        <Button
          type="button"
          size="small"
          disabled={this.props.disabled}
          onClick={this.open}
        >
          Recovery history
        </Button>
        {open && (
          <Dialog
            id="discard-recovery"
            title="Recovery history"
            onDismissed={this.close}
            onSubmit={this.close}
            dismissDisabled={busy}
            disabled={busy}
          >
            <DialogContent>
              <p>
                Restore discarded changes or recover an interrupted operation.
                Newer edits are protected.
              </p>
              {loading && <p>Loading recovery history…</p>}
              {error !== null && <p role="alert">{error}</p>}
              {!loading && error === null && records.length === 0 && (
                <p>No recovery records.</p>
              )}
              <ul>
                {records.map(record => (
                  <li key={record.id}>
                    <p>
                      {record.state === 'invalid'
                        ? 'Unreadable recovery record'
                        : record.purpose === 'discard'
                          ? 'Discarded changes'
                          : 'Conflict resolution'}
                      {record.createdMs > 0 &&
                        ` — ${new Date(record.createdMs).toLocaleString()}`}
                      {record.state === 'prepared' && ' — interrupted'}
                      {record.state === 'undone' && ' — restored'}
                    </p>
                    {record.state === 'invalid' && <code>{record.id}</code>}
                    {[...new Set(record.files)].map(file => (
                      <div key={file}>
                        <bdi
                          style={{
                            whiteSpace: 'pre-wrap',
                            overflowWrap: 'anywhere',
                          }}
                        >
                          {file}
                        </bdi>
                      </div>
                    ))}
                    {record.unavailableLfs > 0 && (
                      <p>Some Git LFS content is unavailable.</p>
                    )}
                    {(record.state === 'applied' ||
                      record.state === 'prepared') && (
                      <Button
                        type="button"
                        size="small"
                        disabled={busy || loading || confirming !== null}
                        onClick={() => {
                          void this.restore(record.id)
                        }}
                      >
                        {record.state === 'prepared' ? 'Recover' : 'Undo'}
                      </Button>
                    )}
                    {(record.state === 'applied' ||
                      record.state === 'undone') &&
                      (confirming === record.id ? (
                        <div>
                          <p>
                            This permanently removes the saved recovery data.
                            This cannot be undone.
                          </p>
                          <Button
                            type="button"
                            size="small"
                            disabled={busy || loading}
                            onClick={() => {
                              void this.forget(record.id)
                            }}
                          >
                            Delete permanently
                          </Button>
                          <Button
                            type="button"
                            size="small"
                            disabled={busy || loading}
                            onClick={() =>
                              this.setState({ confirming: null, error: null })
                            }
                          >
                            Cancel
                          </Button>
                        </div>
                      ) : (
                        <Button
                          type="button"
                          size="small"
                          disabled={busy || loading || confirming !== null}
                          onClick={() =>
                            this.setState({
                              confirming: record.id,
                              error: null,
                            })
                          }
                        >
                          Delete backup
                        </Button>
                      ))}
                  </li>
                ))}
              </ul>
            </DialogContent>
            <DefaultDialogFooter disabled={busy} />
          </Dialog>
        )}
      </>
    )
  }
}
