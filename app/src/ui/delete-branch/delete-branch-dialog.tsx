import * as React from 'react'

import { Dispatcher } from '../dispatcher'
import { Repository } from '../../models/repository'
import { Branch } from '../../models/branch'
import { Checkbox, CheckboxValue } from '../lib/checkbox'
import { Dialog, DialogContent, DialogFooter } from '../dialog'
import { Ref } from '../lib/ref'
import { OkCancelButtonGroup } from '../dialog/ok-cancel-button-group'
import { backend, type IRemoteBranchDeletionReview } from '../../backend/client'
import { RemoteDeletionRecovery, RemoteDeletionReview } from './remote-deletion-review'

interface IDeleteBranchProps {
  readonly dispatcher: Dispatcher
  readonly repository: Repository
  readonly branch: Branch
  readonly existsOnRemote: boolean
  readonly onDismissed: () => void
  readonly onDeleted: (repository: Repository) => void
}

interface IDeleteBranchState {
  readonly includeRemoteBranch: boolean
  readonly isDeleting: boolean
  readonly reviewing: boolean
  readonly review: IRemoteBranchDeletionReview | null
  readonly error: string | null
  readonly receipt: string | null
  readonly warnings: ReadonlyArray<string>
}

export class DeleteBranch extends React.Component<
  IDeleteBranchProps,
  IDeleteBranchState
> {
  private reviewGeneration = 0
  private submitting = false
  public constructor(props: IDeleteBranchProps) {
    super(props)

    this.state = {
      includeRemoteBranch: false,
      isDeleting: false,
      reviewing: false,
      review: null,
      error: null,
      receipt: null,
      warnings: [],
    }
  }

  public componentWillUnmount() { this.reviewGeneration++ }

  public render() {
    return (
      <Dialog
        id="delete-branch"
        title={__DARWIN__ ? 'Delete Branch' : 'Delete branch'}
        type="warning"
        onSubmit={this.deleteBranch}
        onDismissed={this.props.onDismissed}
        disabled={this.state.isDeleting}
        loading={this.state.isDeleting}
        role="alertdialog"
        ariaDescribedBy="delete-branch-confirmation-message delete-branch-confirmation-message-remote"
      >
        <DialogContent>
          <div id="delete-branch-confirmation-message">
            <p>
              Delete branch <Ref>{this.props.branch.name}</Ref>?
            </p>
            <p>Deleting a branch can affect collaborators and local work.</p>

            {this.renderDeleteOnRemote()}
            {this.state.reviewing && <p>Reviewing push destinations…</p>}
            {this.state.error && <p role="alert">{this.state.error}</p>}
            {this.state.error && this.state.includeRemoteBranch && this.state.receipt === null && <button type="button" disabled={this.state.reviewing} onClick={this.reviewBranch}>Review destinations again</button>}
            {this.state.review && <RemoteDeletionReview review={this.state.review} />}
            {this.state.receipt && <p>Deletion receipt: {this.state.receipt}</p>}
            {this.state.warnings.map((warning, index) => <p role="alert" key={index}>{warning}</p>)}
            {backend().transportKind !== 'none' && <RemoteDeletionRecovery repository={this.props.repository} dispatcher={this.props.dispatcher} />}
          </div>
        </DialogContent>
        <DialogFooter>
          <OkCancelButtonGroup destructive={true} okButtonText="Delete" okButtonDisabled={this.state.receipt !== null || this.state.reviewing || (backend().transportKind !== 'none' && this.state.includeRemoteBranch && (this.state.review === null || this.state.review.endpoints.some(endpoint => !endpoint.displaySafe)))} />
        </DialogFooter>
      </Dialog>
    )
  }

  private renderDeleteOnRemote() {
    if (this.props.branch.upstreamRemoteName && this.props.existsOnRemote) {
      return (
        <div>
          <p id="delete-branch-confirmation-message-remote">
            <strong>
              The branch also exists on the remote, do you wish to delete it
              there as well?
            </strong>
          </p>
          <Checkbox
            label="Yes, delete this branch on the remote"
            value={
              this.state.includeRemoteBranch
                ? CheckboxValue.On
                : CheckboxValue.Off
            }
            onChange={this.onIncludeRemoteChanged}
          />
        </div>
      )
    }

    return null
  }

  private onIncludeRemoteChanged = (
    event: React.FormEvent<HTMLInputElement>
  ) => {
    const value = event.currentTarget.checked

    this.reviewGeneration++

    this.setState({ includeRemoteBranch: value, review: null, error: null, reviewing: false }, () => {
      if (value && backend().transportKind !== 'none') void this.reviewBranch()
    })
  }

  private reviewBranch = async () => {
    const generation = ++this.reviewGeneration
    this.setState({ reviewing: true, error: null, review: null })
    try {
      const review = await this.props.dispatcher.previewRemoteBranchDeletion(this.props.repository, this.props.branch)
      if (generation === this.reviewGeneration) this.setState(state => ({ reviewing: false, review: state.includeRemoteBranch ? review : null }))
    } catch { if (generation === this.reviewGeneration) this.setState({ reviewing: false, error: 'Could not review every push destination. Refresh and try again.' }) }
  }

  private deleteBranch = async () => {
    const { dispatcher, repository, branch } = this.props

    if (this.submitting || this.state.receipt !== null || (backend().transportKind !== 'none' && this.state.includeRemoteBranch && this.state.review === null)) return
    this.submitting = true
    this.setState({ isDeleting: true })
    try {
      if (backend().transportKind !== 'none' && this.state.includeRemoteBranch && this.state.review !== null) {
        const result = await dispatcher.deleteReviewedRemoteBranch(repository, this.state.review)
        this.setState({ receipt: result.receipt, warnings: result.warnings, isDeleting: false, review: null })
        this.props.onDeleted(repository)
      } else {
        await dispatcher.deleteLocalBranch(repository, branch, this.state.includeRemoteBranch)
        this.props.onDeleted(repository)
        this.props.onDismissed()
      }
    } catch {
      this.setState({ isDeleting: false, review: null, error: 'Deletion may be incomplete. Inspect local and remote branches and deletion recovery before retrying.' })
    } finally {
      this.submitting = false
    }
  }
}
