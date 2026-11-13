// `keytar` for the webview build: secrets live in the Rust shell's
// credential store and never in web storage.
const invoke = (operation: string, options: object) =>
  (globalThis as any).__desktop.invoke(
    `desktop.io.keychain-${operation}`,
    options
  )

export function getPassword(
  service: string,
  account: string
): Promise<string | null> {
  return invoke('get', { service, account })
}

export async function setPassword(
  service: string,
  account: string,
  password: string
): Promise<void> {
  await invoke('set', { service, account, password })
}

export function deletePassword(
  service: string,
  account: string
): Promise<boolean> {
  return invoke('delete', { service, account })
}

export default { getPassword, setPassword, deletePassword }
