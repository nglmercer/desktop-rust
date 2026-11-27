export type DesktopNotificationPermission = 'granted' | 'denied' | 'default'
export type DesktopNotificationEvent = 'click'
export type NotificationCallback<
  T extends Record<string, any> = Record<string, any>,
> = (event: DesktopNotificationEvent, id: string, userInfo: T) => void
