export class Module {
  createRequire() {
    return {
      resolvePath() {
        return ''
      },
    }
  }
}
export const _nodeModulePaths = (p: string) => [p]
export function createRequire(_url?: string) {
  return { resolve: (id: string) => id, resolvePath: () => '' }
}
export const createRequire_ = createRequire
