import * as React from 'react'
import { DialogContent } from '../dialog'
import { Checkbox, CheckboxValue } from '../lib/checkbox'
import { LinkButton } from '../lib/link-button'
import {
  readNotificationSettings,
  requestNotificationSettingsPermission,
  type NotificationSettings,
} from '../../lib/notifications/notification-settings'

interface INotificationPreferencesProps {
  readonly notificationsEnabled: boolean
  readonly onNotificationsEnabledChanged: (checked: boolean) => void
}

interface INotificationPreferencesState {
  readonly settings: NotificationSettings
}

export class Notifications extends React.Component<
  INotificationPreferencesProps,
  INotificationPreferencesState
> {
  private mounted = false
  private readGeneration = 0
  private requestingPermission = false

  public constructor(props: INotificationPreferencesProps) {
    super(props)

    this.state = {
      settings: { kind: 'unavailable' },
    }
  }

  public componentDidMount() {
    this.mounted = true
    window.addEventListener('focus', this.onWindowFocus)
    void this.updateNotificationsState()
  }

  public componentWillUnmount() {
    this.mounted = false
    this.readGeneration++
    window.removeEventListener('focus', this.onWindowFocus)
  }

  private onWindowFocus = () => {
    void this.updateNotificationsState()
  }

  private onNotificationsEnabledChanged = (
    event: React.FormEvent<HTMLInputElement>
  ) => {
    this.props.onNotificationsEnabledChanged(event.currentTarget.checked)
  }

  public render() {
    return (
      <DialogContent>
        <div className="advanced-section">
          <h2>Notifications</h2>
          <Checkbox
            label="Enable notifications"
            value={
              this.props.notificationsEnabled
                ? CheckboxValue.On
                : CheckboxValue.Off
            }
            onChange={this.onNotificationsEnabledChanged}
          />
          <div className="settings-description">
            Allows the display of notifications when high-signal events take
            place in the current repository.{this.renderNotificationHint()}
          </div>
        </div>
      </DialogContent>
    )
  }

  private onGrantNotificationPermission = async () => {
    if (this.requestingPermission) {
      return
    }
    this.requestingPermission = true
    try {
      await requestNotificationSettingsPermission()
    } catch (error) {
      log.warn('Failed to request notification permission', error)
    } finally {
      this.requestingPermission = false
      if (this.mounted) {
        void this.updateNotificationsState()
      }
    }
  }

  private async updateNotificationsState() {
    const generation = ++this.readGeneration
    try {
      const settings = await readNotificationSettings()
      if (this.mounted && this.readGeneration === generation) {
        this.setState({ settings })
      }
    } catch (error) {
      log.warn('Failed to read notification settings', error)
      if (this.mounted && this.readGeneration === generation) {
        this.setState({ settings: { kind: 'unavailable' } })
      }
    }
  }

  private renderNotificationHint() {
    // No need to bother the user if their environment doesn't support our
    // notifications or if they've been explicitly disabled.
    const { settings } = this.state
    if (settings.kind === 'unavailable' || !this.props.notificationsEnabled) {
      return null
    }

    if (settings.canRequest && settings.permission === 'default') {
      return (
        <>
          {' '}
          You need to{' '}
          <LinkButton onClick={this.onGrantNotificationPermission}>
            grant permission
          </LinkButton>{' '}
          to display these notifications from GitHub Desktop.
        </>
      )
    }

    if (settings.kind === 'web') {
      return settings.permission === 'denied' ? (
        <div className="setting-hint-warning">
          <span className="warning-icon">⚠️</span> Notifications are blocked.
          Enable them in your browser or system notification settings.
        </div>
      ) : null
    }

    if (settings.settingsUrl === null) {
      return null
    }

    if (settings.permission === 'denied') {
      return (
        <div className="setting-hint-warning">
          <span className="warning-icon">⚠️</span> GitHub Desktop has no
          permission to display notifications. Please, enable them in the{' '}
          <LinkButton uri={settings.settingsUrl}>
            Notifications Settings
          </LinkButton>
          .
        </div>
      )
    }

    const verb =
      settings.permission === 'granted' ? 'properly configured' : 'enabled'

    return (
      <>
        {' '}
        Make sure notifications are {verb} for GitHub Desktop in the{' '}
        <LinkButton uri={settings.settingsUrl}>
          Notifications Settings
        </LinkButton>
        .
      </>
    )
  }
}
