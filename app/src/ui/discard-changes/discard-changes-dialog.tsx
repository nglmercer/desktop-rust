import * as React from 'react'

import type { Repository } from '../../models/repository'
import type { Dispatcher } from '../dispatcher'
import type { WorkingDirectoryFileChange } from '../../models/status'
import { Dialog, DialogContent, DialogFooter } from '../dialog'
import { PathText } from '../lib/path-text'
import { Checkbox, CheckboxValue } from '../lib/checkbox'
import { TrashNameLabel } from '../lib/context-menu'
import { OkCancelButtonGroup } from '../dialog/ok-cancel-button-group'
import { backend } from '../../backend/client'

interface IDiscardChangesProps {
  readonly repository: Repository
  readonly dispatcher: Dispatcher
  readonly files: ReadonlyArray<WorkingDirectoryFileChange>
  readonly confirmDiscardChanges: boolean
  /**
   * Determines whether to show the option
   * to ask for confirmation when discarding
   * changes
   */
  readonly discardingAllChanges: boolean
  readonly showDiscardChangesSetting: boolean
  readonly onDismissed: () => void
  readonly onConfirmDiscardChangesChanged: (optOut: boolean) => void
}

interface IDiscardChangesState {
  /**
   * Whether or not we're currently in the process of discarding
   * changes. This is used to display a loading state
   */
  readonly isDiscardingChanges: boolean

  readonly confirmDiscardChanges: boolean
  readonly nativePreview: {
    readonly basis: string
    readonly files: ReadonlyArray<string>
  } | null
  readonly nativeError: string | null
  readonly loadingPreview: boolean
}

/**
 * If we're discarding any more than this number, we won't bother listing them
 * all.
 */
const MaxFilesToList = 10

/** A component to confirm and then discard changes. */
export class DiscardChanges extends React.Component<
  IDiscardChangesProps,
  IDiscardChangesState
> {
  private readonly useNative =
    typeof window !== 'undefined' && backend().transportKind !== 'none'
  private mounted = false
  private previewGeneration = 0
  public constructor(props: IDiscardChangesProps) {
    super(props)

    this.state = {
      isDiscardingChanges: false,
      confirmDiscardChanges: this.props.confirmDiscardChanges,
      nativePreview: null,
      nativeError: null,
      loadingPreview: this.useNative,
    }
  }

  public componentDidMount() {
    this.mounted = true
    if (this.useNative) void this.loadNativePreview()
  }

  public componentDidUpdate(prevProps: IDiscardChangesProps) {
    if (
      this.useNative &&
      (prevProps.repository.path !== this.props.repository.path ||
        prevProps.files !== this.props.files)
    )
      void this.loadNativePreview()
  }

  public componentWillUnmount() {
    this.mounted = false
    this.previewGeneration++
  }

  private async loadNativePreview() {
    const generation = ++this.previewGeneration
    this.setState({
      loadingPreview: true,
      nativeError: null,
      nativePreview: null,
    })
    try {
      const preview = await backend().repositoryDiscardBatchPreview(
        this.props.repository.path,
        this.props.files.map(file => file.path)
      )
      if (this.mounted && generation === this.previewGeneration)
        this.setState({ nativePreview: preview, loadingPreview: false })
    } catch (error) {
      if (this.mounted && generation === this.previewGeneration)
        this.setState({
          loadingPreview: false,
          nativeError:
            error instanceof Error
              ? error.message
              : 'Could not review these changes.',
        })
    }
  }

  private getOkButtonLabel() {
    if (this.props.discardingAllChanges) {
      return __DARWIN__ ? 'Discard All Changes' : 'Discard all changes'
    }
    return __DARWIN__ ? 'Discard Changes' : 'Discard changes'
  }

  private getDialogTitle() {
    if (this.props.discardingAllChanges) {
      return __DARWIN__
        ? 'Confirm Discard All Changes'
        : 'Confirm discard all changes'
    }
    return __DARWIN__ ? 'Confirm Discard Changes' : 'Confirm discard changes'
  }

  public render() {
    const isDiscardingChanges = this.state.isDiscardingChanges

    return (
      <Dialog
        id="discard-changes"
        title={this.getDialogTitle()}
        onDismissed={this.props.onDismissed}
        onSubmit={this.discard}
        dismissDisabled={isDiscardingChanges}
        loading={isDiscardingChanges}
        disabled={isDiscardingChanges}
        type="warning"
        role="alertdialog"
        ariaDescribedBy="discard-changes-confirmation-file-list discard-changes-confirmation-message"
      >
        <DialogContent>
          {this.renderFileList()}
          <p id="discard-changes-confirmation-message">
            {this.useNative ? (
              'A backup will be available in Recovery history.'
            ) : (
              <>
                Changes can be restored by retrieving them from the{' '}
                {TrashNameLabel}.
              </>
            )}
          </p>
          {this.renderConfirmDiscardChanges()}
        </DialogContent>

        <DialogFooter>
          <OkCancelButtonGroup
            destructive={true}
            okButtonText={this.getOkButtonLabel()}
            okButtonDisabled={
              isDiscardingChanges ||
              (this.useNative && this.state.nativePreview === null)
            }
            cancelButtonDisabled={isDiscardingChanges}
          />
        </DialogFooter>
      </Dialog>
    )
  }

  private renderConfirmDiscardChanges() {
    if (this.props.showDiscardChangesSetting) {
      return (
        <Checkbox
          label="Do not show this message again"
          value={
            this.state.confirmDiscardChanges
              ? CheckboxValue.Off
              : CheckboxValue.On
          }
          onChange={this.onConfirmDiscardChangesChanged}
        />
      )
    } else {
      // since we ignore the users option to not show
      // confirmation, we don't want to show a checkbox
      // that will have no effect
      return null
    }
  }

  private renderFileList() {
    if (this.useNative) {
      if (this.state.loadingPreview)
        return (
          <p id="discard-changes-confirmation-file-list">Reviewing changes…</p>
        )
      if (this.state.nativeError !== null)
        return (
          <p id="discard-changes-confirmation-file-list" role="alert">
            {this.state.nativeError}
          </p>
        )
      return (
        <div id="discard-changes-confirmation-file-list">
          <p>Discard changes to these files?</p>
          <div className="file-list">
            <ul>
              {this.state.nativePreview?.files.map(path => (
                <li key={path}>
                  <bdi
                    style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
                  >
                    {path}
                  </bdi>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )
    }
    if (this.props.files.length > MaxFilesToList) {
      return (
        <p id="discard-changes-confirmation-file-list">
          Are you sure you want to discard all {this.props.files.length} changed
          files?
        </p>
      )
    } else {
      return (
        <div id="discard-changes-confirmation-file-list">
          <p>Are you sure you want to discard all changes to:</p>
          <div className="file-list">
            <ul>
              {this.props.files.map(p => (
                <li key={p.id}>
                  <PathText path={p.path} />
                </li>
              ))}
            </ul>
          </div>
        </div>
      )
    }
  }

  private discard = async () => {
    if (this.useNative && this.state.nativePreview === null) return
    this.setState({ isDiscardingChanges: true })

    await this.props.dispatcher.discardChanges(
      this.props.repository,
      this.props.files,
      true,
      this.state.nativePreview?.basis
    )

    this.props.onConfirmDiscardChangesChanged(this.state.confirmDiscardChanges)
    this.props.onDismissed()
  }

  private onConfirmDiscardChangesChanged = (
    event: React.FormEvent<HTMLInputElement>
  ) => {
    const value = !event.currentTarget.checked

    this.setState({ confirmDiscardChanges: value })
  }
}
