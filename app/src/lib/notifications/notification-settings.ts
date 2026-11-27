import {
  getNativeNotificationSettingsUrl,
  supportsNativeNotificationPermissionRequest,
  supportsNativeNotifications,
} from './native-notification-capabilities'
import {
  getNotificationsPermission,
  requestNotificationsPermission,
} from '../../ui/main-process-proxy'

export type NotificationSettings =
  | { readonly kind: 'unavailable' }
  | {
      readonly kind: 'native' | 'web'
      readonly permission: NotificationPermission
      readonly canRequest: boolean
      readonly settingsUrl: string | null
    }

/** Read the permission from the implementation that will display notifications. */
export async function readNotificationSettings(): Promise<NotificationSettings> {
  if (supportsNativeNotifications()) {
    return {
      kind: 'native',
      permission: await getNotificationsPermission(),
      canRequest: supportsNativeNotificationPermissionRequest(),
      settingsUrl: getNativeNotificationSettingsUrl(),
    }
  }
  if (typeof Notification === 'undefined') {
    return { kind: 'unavailable' }
  }
  return {
    kind: 'web',
    permission: Notification.permission,
    canRequest: typeof Notification.requestPermission === 'function',
    settingsUrl: null,
  }
}

/** Request permission synchronously from a user-initiated click. */
export function requestNotificationSettingsPermission(): Promise<unknown> {
  if (supportsNativeNotifications()) {
    return requestNotificationsPermission()
  }
  if (
    typeof Notification === 'undefined' ||
    typeof Notification.requestPermission !== 'function'
  ) {
    return Promise.resolve()
  }
  return Notification.requestPermission()
}
