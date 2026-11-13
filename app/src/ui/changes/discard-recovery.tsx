import * as React from 'react'
import {
  backend,
  type CommitCheckpoint,
  type CommitCheckpointInspection,
  type IDiscardRecoveryRecord,
} from '../../backend/client'
import type { Repository } from '../../models/repository'
import type { Dispatcher } from '../dispatcher'
import { PopupType } from '../../models/popup'
import { DefaultDialogFooter, Dialog, DialogContent } from '../dialog'
import { Button } from '../lib/button'

interface IProps {
  readonly repository: Repository
  readonly dispatcher: Dispatcher
  readonly disabled: boolean
}

interface IDialogProps {
  readonly repository: Repository
  readonly dispatcher: Dispatcher
  readonly onDismissed: () => void
}

export function DiscardRecovery({ repository, dispatcher, disabled }: IProps) {
  return (
    <Button
      type="button"
      size="small"
      disabled={disabled}
      onClick={() => {
        void dispatcher.showPopup({
          type: PopupType.RecoveryHistory,
          repository,
        })
      }}
    >
      Recovery history
    </Button>
  )
}

interface IState {
  readonly loading: boolean
  readonly busy: boolean
  readonly records: ReadonlyArray<IDiscardRecoveryRecord>
  readonly commits: ReadonlyArray<CommitCheckpoint>
  readonly error: string | null
  readonly commitError: string | null
  readonly confirming: string | null
  readonly inspection: CommitCheckpointInspection | null
}

/** Durable recovery remains available after the immediate Undo banner closes. */
export class DiscardRecoveryDialog extends React.Component<
  IDialogProps,
  IState
> {
  private generation = 0
  private mounted = false
  private operationInFlight = false
  public state: IState = {
    loading: false,
    busy: false,
    records: [],
    commits: [],
    error: null,
    commitError: null,
    confirming: null,
    inspection: null,
  }

  public componentDidMount() {
    this.mounted = true
    void this.load()
  }
  public componentWillUnmount() {
    this.mounted = false
    this.generation++
  }

  private close = () => {
    if (this.operationInFlight) return
    this.generation++
    this.props.onDismissed()
  }

  private async load() {
    const generation = ++this.generation
    this.setState({ loading: true, error: null, commitError: null })
    try {
      const [discardResult, commitResult] = await Promise.all([
        backend()
          .repositoryDiscardHistory(this.props.repository.path)
          .then(
            records => ({ records, error: null }),
            error => ({
              records: [],
              error:
                error instanceof Error
                  ? error.message
                  : 'Could not load discard recovery history.',
            })
          ),
        backend()
          .listCommitCheckpoints(this.props.repository.path)
          .then(
            commits => ({ commits, error: null }),
            error => ({
              commits: [],
              error:
                error instanceof Error
                  ? error.message
                  : 'Could not load commit recovery history.',
            })
          ),
      ])
      if (this.mounted && generation === this.generation)
        this.setState({
          records: discardResult.records,
          error: discardResult.error,
          commits: commitResult.commits,
          commitError: commitResult.error,
          loading: false,
        })
    } catch (error) {
      if (this.mounted && generation === this.generation)
        this.setState({
          loading: false,
          records: [],
          commits: [],
          error:
            error instanceof Error
              ? error.message
              : 'Could not load recovery history.',
        })
    }
  }

  private recoverCommit = async (id: string) => {
    if (this.operationInFlight) return
    this.operationInFlight = true
    this.setState({ busy: true, error: null })
    try {
      await this.props.dispatcher.recoverPublishedCommitCheckpoint(
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
              : 'Could not recover this commit.',
        })
    } finally {
      this.operationInFlight = false
    }
  }

  private closeCommit = async (id: string) => {
    if (this.operationInFlight) return
    this.operationInFlight = true
    this.setState({ busy: true, error: null })
    try {
      await this.props.dispatcher.closeUnchangedCommitCheckpoint(
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
              : 'Could not close this commit attempt.',
        })
    } finally {
      this.operationInFlight = false
    }
  }

  private forgetPreparedCommit = async (id: string) => {
    if (
      this.operationInFlight ||
      this.state.confirming !== `commit:${id}` ||
      !this.state.commits.some(
        commit => commit.id === id && commit.state === 'prepared'
      )
    )
      return
    this.operationInFlight = true
    this.setState({ busy: true, commitError: null })
    try {
      await this.props.dispatcher.forgetPreparedCommitCheckpoint(
        this.props.repository,
        id
      )
      if (this.mounted) {
        this.setState({ busy: false, confirming: null, inspection: null })
        await this.load()
      }
    } catch (error) {
      if (this.mounted)
        this.setState({
          busy: false,
          commitError:
            error instanceof Error
              ? error.message
              : 'Could not discard this prepared attempt.',
        })
    } finally {
      this.operationInFlight = false
    }
  }

  private inspectCommit = async (id: string) => {
    if (this.operationInFlight) return
    this.operationInFlight = true
    this.setState({ busy: true, commitError: null, inspection: null })
    try {
      const inspection = await backend().inspectCommitCheckpoint(
        this.props.repository.path,
        id
      )
      if (this.mounted) {
        this.setState({ busy: false, inspection })
        if (inspection.state !== 'prepared') await this.load()
      }
    } catch (error) {
      if (this.mounted)
        this.setState({
          busy: false,
          commitError:
            error instanceof Error
              ? error.message
              : 'Could not inspect this commit attempt.',
        })
    } finally {
      this.operationInFlight = false
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
    const {
      loading,
      busy,
      records,
      commits,
      error,
      commitError,
      confirming,
      inspection,
    } = this.state
    const interruptedCommits = commits.filter(
      record =>
        record.state === 'prepared' ||
        record.state === 'publishing' ||
        record.state === 'published' ||
        record.state === 'invalid'
    )
    return (
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
            Restore discarded changes or recover an interrupted operation. Newer
            edits are protected.
          </p>
          {loading && <p>Loading recovery history…</p>}
          {error !== null && <p role="alert">{error}</p>}
          {commitError !== null && <p role="alert">{commitError}</p>}
          {!loading &&
            error === null &&
            commitError === null &&
            records.length === 0 &&
            interruptedCommits.length === 0 && <p>No recovery records.</p>}
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
                {(record.state === 'applied' || record.state === 'undone') &&
                  (confirming === record.id ? (
                    <div>
                      <p>
                        This permanently removes the saved recovery data. This
                        cannot be undone.
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
          {interruptedCommits.length > 0 && (
            <section aria-label="Commit recovery">
              <h3>Interrupted commits</h3>
              <p>
                Recovery checks the branch and staged changes before finishing
                the commit.
              </p>
              <ul>
                {interruptedCommits.map(record => (
                  <li key={record.id}>
                    <p>
                      {record.state === 'prepared'
                        ? 'Prepared commit was not published'
                        : record.state === 'invalid'
                          ? 'Unreadable commit checkpoint'
                          : 'Commit needs inspection'}
                    </p>
                    <code>{record.id}</code>
                    {record.state !== 'invalid' &&
                      record.state !== 'prepared' && (
                        <Button
                          type="button"
                          size="small"
                          disabled={busy || loading || confirming !== null}
                          onClick={() => {
                            void this.recoverCommit(record.id)
                          }}
                        >
                          Recover commit
                        </Button>
                      )}
                    {record.state === 'prepared' && (
                      <>
                        <Button
                          type="button"
                          size="small"
                          disabled={busy || loading || confirming !== null}
                          onClick={() => {
                            void this.inspectCommit(record.id)
                          }}
                        >
                          Inspect prepared attempt
                        </Button>
                        {inspection?.id === record.id && (
                          <p>
                            Current state: {inspection.state}. Branch{' '}
                            {inspection.branch_matches ? 'matches' : 'changed'};
                            HEAD{' '}
                            {inspection.head_matches ? 'matches' : 'changed'};
                            index{' '}
                            {inspection.index_matches ? 'matches' : 'changed'};
                            repository operation{' '}
                            {inspection.operation_matches
                              ? 'matches'
                              : 'changed'}
                            .
                          </p>
                        )}
                        {confirming === `commit:${record.id}` ? (
                          <div>
                            <p>
                              Discard this prepared attempt and its backup? No
                              commit was published by this attempt.
                            </p>
                            <Button
                              type="button"
                              size="small"
                              disabled={busy || loading}
                              onClick={() => {
                                void this.forgetPreparedCommit(record.id)
                              }}
                            >
                              Discard prepared attempt
                            </Button>
                            <Button
                              type="button"
                              size="small"
                              disabled={busy || loading}
                              onClick={() =>
                                this.setState({
                                  confirming: null,
                                  commitError: null,
                                })
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
                                confirming: `commit:${record.id}`,
                                commitError: null,
                              })
                            }
                          >
                            Remove prepared attempt
                          </Button>
                        )}
                      </>
                    )}
                    {record.state === 'publishing' && (
                      <>
                        <p>
                          Close this attempt only if the branch and index are
                          unchanged. Its backup will be kept; the commit may
                          have been published and later reversed.
                        </p>
                        <Button
                          type="button"
                          size="small"
                          disabled={busy || loading || confirming !== null}
                          onClick={() => {
                            void this.closeCommit(record.id)
                          }}
                        >
                          Close unchanged attempt
                        </Button>
                      </>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          )}
        </DialogContent>
        <DefaultDialogFooter disabled={busy} />
      </Dialog>
    )
  }
}
