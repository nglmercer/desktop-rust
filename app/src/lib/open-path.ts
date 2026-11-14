/** Open a local file with the desktop host's default application. */
export async function openPath(path: string): Promise<string> {
  const bridge: unknown = Reflect.get(window, '__desktop')
  if (bridge !== undefined && bridge !== null) {
    if (
      typeof bridge !== 'object' ||
      Reflect.get(bridge, 'platform') !== 'wry' ||
      typeof Reflect.get(bridge, 'invoke') !== 'function'
    ) {
      return 'Opening a host file is unavailable from this browser.'
    }
    try {
      const invoke = Reflect.get(bridge, 'invoke')
      const result: unknown = await invoke.call(
        bridge,
        'desktop.io.open-path',
        {
          path,
        }
      )
      return typeof result === 'string' ? result : 'Unable to open the path.'
    } catch {
      return 'Unable to confirm that the path was opened. The application may already be open; check before retrying.'
    }
  }
  return 'Opening a host file is unavailable from this browser.'
}
