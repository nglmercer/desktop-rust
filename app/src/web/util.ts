export function promisify(fn:any){ return (...args:any[])=> new Promise((_res,rej)=> rej(new Error('util.promisify shim — use Rust bridge')) ) }
export function inherits(){}
export function inspect(o:any){ return JSON.stringify(o) }
export function format(...a:any[]){ return a.join(' ') }
export const types = {}
