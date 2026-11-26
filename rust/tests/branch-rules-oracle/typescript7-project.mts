import assert from 'node:assert/strict'
import path from 'node:path'
import { API, type Project } from 'typescript/unstable/sync'

export interface ITypeScriptProject {
  readonly api: API
  readonly project: Project
}

/** Open the repository's root project once with the native TypeScript checker. */
export function openTypeScriptProject(root: string): ITypeScriptProject {
  const api = new API({ cwd: root })
  try {
    const snapshot = api.updateSnapshot({
      openProjects: [path.join(root, 'tsconfig.json')],
    })
    const project = snapshot.getProjects()[0]
    assert(project, 'Could not open the root TypeScript project')
    return { api, project }
  } catch (error) {
    api.close()
    throw error
  }
}
