/**
 * Native notification capability facts for the Rust host.
 *
 * The retired desktop-notifications integration reported unsupported in this
 * renderer, causing notification delivery to use the web Notification API.
 * Keep that behavior explicit here until native delivery is implemented as a
 * typed host operation.
 */
export const supportsNativeNotifications = () => false
export const supportsNativeNotificationPermissionRequest = () => false
export const getNativeNotificationSettingsUrl = (): string | null => null
