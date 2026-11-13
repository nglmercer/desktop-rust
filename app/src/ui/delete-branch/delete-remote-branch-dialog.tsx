import * as React from 'react'

import { Dispatcher } from '../dispatcher'
import { Repository } from '../../models/repository'
import { Branch } from '../../models/branch'
import { Dialog, DialogContent, DialogFooter } from '../dialog'
import { Ref } from '../lib/ref'
import { OkCancelButtonGroup } from '../dialog/ok-cancel-button-group'
import { backend, type IRemoteBranchDeletionReview } from '../../backend/client'
import { RemoteDeletionRecovery, RemoteDeletionReview } from './remote-deletion-review'

interface IDeleteRemoteBranchProps {
  readonly dispatcher: Dispatcher
  readonly repository: Repository
  readonly branch: Branch
  readonly onDismissed: () => void
  readonly onDeleted: (repository: Repository) => void
}
interface IDeleteRemoteBranchState {
  readonly isDeleting: boolean
  readonly reviewing: boolean
  readonly review: IRemoteBranchDeletionReview | null
  readonly error: string | null
  readonly receipt: string | null
  readonly warnings: ReadonlyArray<string>
}
export class DeleteRemoteBranch extends React.Component<
  IDeleteRemoteBranchProps,
  IDeleteRemoteBranchState
> {
  private reviewGeneration = 0
  private submitting = false
  public constructor(props: IDeleteRemoteBranchProps) {
    super(props)

    this.state = {
      isDeleting: false,
      reviewing: false,
      review: null,
      error: null,
      receipt: null,
      warnings: [],
    }
  }

  public componentDidMount() {
    if (backend().transportKind !== 'none') void this.reviewBranch()
  }

  public componentWillUnmount() { this.reviewGeneration++ }

  private reviewBranch = async () => {
    const generation = ++this.reviewGeneration
    this.setState({ reviewing: true, error: null, review: null })
    try {
      const review = await this.props.dispatcher.previewRemoteBranchDeletion(this.props.repository, this.props.branch)
      if (generation === this.reviewGeneration) this.setState({ review, reviewing: false })
    } catch { if (generation === this.reviewGeneration) this.setState({ reviewing: false, error: 'Could not review every push destination. Refresh and try again.' }) }
  }

  public render() {
    return (
      <Dialog
        id="delete-branch"
        title={__DARWIN__ ? 'Delete Remote Branch' : 'Delete remote branch'}
        type="warning"
        onSubmit={this.deleteBranch}
        onDismissed={this.props.onDismissed}
        disabled={this.state.isDeleting}
        loading={this.state.isDeleting}
        role="alertdialog"
        ariaDescribedBy="delete-branch-confirmation-message"
      >
        <DialogContent>
          <div id="delete-branch-confirmation-message">
            <p>
              Delete remote branch <Ref>{this.props.branch.name}</Ref>?
            </p>
            <p>Remote deletion can affect collaborators. A retained tip can only be restored as a local branch.</p>

            <p>
              This branch does not exist locally. Deleting it may impact others
              collaborating on this branch.
            </p>
          </div>
          {this.state.reviewing && <p>Reviewing push destinations…</p>}
          {this.state.error && <p role="alert">{this.state.error}</p>}
          {this.state.error && this.state.receipt === null && <button type="button" disabled={this.state.reviewing} onClick={this.reviewBranch}>Review destinations again</button>}
          {this.state.review && <RemoteDeletionReview review={this.state.review} />}
          {this.state.receipt && <p>Deletion receipt: {this.state.receipt}</p>}
          {this.state.warnings.map((warning, index) => <p role="alert" key={index}>{warning}</p>)}
          {backend().transportKind !== 'none' && <RemoteDeletionRecovery repository={this.props.repository} dispatcher={this.props.dispatcher} />}
        </DialogContent>
        <DialogFooter>
          <OkCancelButtonGroup destructive={true} okButtonText="Delete" okButtonDisabled={this.state.receipt !== null || this.state.reviewing || (backend().transportKind !== 'none' && (this.state.review === null || this.state.review.endpoints.some(endpoint => !endpoint.displaySafe)))} />
        </DialogFooter>
      </Dialog>
    )
  }

  private deleteBranch = async () => {
    const { dispatcher, repository, branch } = this.props

    if (this.submitting || this.state.receipt !== null || (backend().transportKind !== 'none' && this.state.review === null)) return
    this.submitting = true

    this.setState({ isDeleting: true })

    try {
      if (backend().transportKind !== 'none') {
        if (this.state.review === null) return
        const result = await dispatcher.deleteReviewedRemoteBranch(repository, this.state.review)
        this.setState({ receipt: result.receipt, warnings: result.warnings, isDeleting: false, review: null })
        this.props.onDeleted(repository)
      } else {
        await dispatcher.deleteRemoteBranch(repository, branch)
        this.props.onDeleted(repository)
        this.props.onDismissed()
      }
    } catch {
      this.setState({ isDeleting: false, review: null, error: 'Deletion may be incomplete. Inspect the remote branch and deletion recovery before retrying.' })
    } finally {
      this.submitting = false
    }
  }
}
