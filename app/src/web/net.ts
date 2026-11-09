export class Server { listen(_p?:any,_cb?:any){ if(typeof _p==='function') (_p as any)(); if(_cb)_cb() } close(_cb?:any){ if(_cb)_cb() } }
export class Socket extends EventTarget { }
export type AddressInfo = { port:number, address:string, family:string }
export function createServer(...args:any[]){ const server = new Server(); if (args.length===0) return server; return server }
export function connect(){ throw new Error('net.connect shim — Rust') }
export function isIP(){ return false }
