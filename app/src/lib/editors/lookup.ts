import { ExternalEditorError } from './shared'
import type { IFoundEditor } from './found-editor'
import { backend } from '../../backend/client'

/**
 * Resolve the installed editors discovered by the Rust host. Host discovery
 * owns platform-specific executable lookup and validation.
 */
export async function getAvailableEditors(): Promise<
  ReadonlyArray<IFoundEditor<string>>
> {
  return backend().availableRustEditors()
}

/**
 * Find an editor installed on the machine using the friendly name, or the
 * first valid editor if `null` is provided.
 *
 * Will throw an error if no editors are found, or if the editor name cannot
 * be found (i.e. it has been removed).
 */
export async function findEditorOrDefault(
  name: string | null
): Promise<IFoundEditor<string> | null> {
  const editors = await getAvailableEditors()
  if (editors.length === 0) {
    return null
  }

  if (name) {
    const match = editors.find(p => p.editor === name) || null
    if (!match) {
      const menuItemName = __DARWIN__ ? 'Settings' : 'Options'
      const message = `The editor '${name}' could not be found. Please open ${menuItemName} and choose an available editor.`

      throw new ExternalEditorError(message, { openPreferences: true })
    }

    return match
  }

  return editors[0]
}
