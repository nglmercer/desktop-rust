export type DesktopNotificationPermission = 'granted' | 'denied' | 'default'
export type DesktopNotificationEvent = 'click'
export type NotificationCallback<
  T extends Record<string, any> = Record<string, any>,
> = (event: DesktopNotificationEvent, id: string, userInfo: T) => void
export interface INotificationOptions {
  readonly toastActivatorClsid?: string
}

export function initializeNotifications(_opts: INotificationOptions): void {}
export function terminateNotifications(): void {}
export function getNotificationsPermission(): Promise<DesktopNotificationPermission> {
  return Promise.resolve('granted')
}
export function requestNotificationsPermission(): Promise<boolean> {
  return Promise.resolve(true)
}
export function showNotification(
  _title: string,
  _body: string,
  _userInfo?: Record<string, any>
): Promise<string | null> {
  return Promise.resolve(null)
}
export function closeNotification(_id: string): void {}
export function supportsNotifications(): boolean {
  return false
}
export function supportsNotificationsPermissionRequest(): boolean {
  return false
}
export function getNotificationSettingsUrl(): string | null {
  return null
}
export function onNotificationEvent<
  T extends Record<string, any> = Record<string, any>,
>(_callback: NotificationCallback<T> | null): void {}
