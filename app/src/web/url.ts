export const URL = globalThis.URL
export function format(urlObj:any){ try{ return new (globalThis.URL)(urlObj).toString() } catch{ return String(urlObj) } }
export function parse(urlStr:string){ try{ const u = new (globalThis.URL)(urlStr); return { protocol: u.protocol, slashes: true, host: u.host, hostname: u.hostname, port: u.port, pathname: u.pathname, search: u.search, query: null, hash: u.hash, href: u.href, auth: null, path: u.pathname+u.search+u.hash } } catch{ return {} } }
export function pathToFileURL(p:string){ return new URL('file://'+p) }
export function fileURLToPath(u:string){ return u.replace('file://','') }
export default { pathToFileURL, fileURLToPath, URL, format, parse }
