export type DesktopNotificationPermission = 'granted' | 'denied' | 'default'
export type NotificationCallback<T> = (args: T) => void
export function getNotificationPermission() {
  return Promise.resolve('granted' as DesktopNotificationPermission)
}
export function requestNotificationPermission() {
  return Promise.resolve(true)
}
export function showNotification(_t: string, _b: string, _u?: any) {
  return Promise.resolve(null)
}
export function supportsNotifications() {
  return false
}
export function supportsNotificationsPermissionRequest() {
  return false
}
export function getNotificationSettingsUrl() {
  return Promise.resolve('')
}
export function onNotificationEvent(_cb: any) {
  return Promise.resolve()
}
export function initializeNotifications() {
  return Promise.resolve()
}
export function closeNotification(_id?: string) {
  return Promise.resolve(0)
}
export function terminateNotifications() {
  return Promise.resolve()
}
export function getNotificationsPermission() {
  return Promise.resolve('granted' as DesktopNotificationPermission)
}
export function requestNotificationsPermission() {
  return Promise.resolve(true)
}
