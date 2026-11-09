export const URL = globalThis.URL
export function pathToFileURL(p:string){ return new URL('file://'+p) }
export function fileURLToPath(u:string){ return u.replace('file://','') }
export default { pathToFileURL, fileURLToPath, URL }
