/**
 * Scoped host secret-store integration. Secrets remain outside web storage;
 * the host owns the platform credential backend behind these operations.
 */
const invoke = (operation: string, options: object) =>
  (globalThis as any).__desktop.invoke(
    `desktop.io.keychain-${operation}`,
    options
  )

export function getHostSecret(
  service: string,
  account: string
): Promise<string | null> {
  return invoke('get', { service, account })
}

export async function setHostSecret(
  service: string,
  account: string,
  secret: string
): Promise<void> {
  await invoke('set', { service, account, password: secret })
}

export function deleteHostSecret(
  service: string,
  account: string
): Promise<boolean> {
  return invoke('delete', { service, account })
}
