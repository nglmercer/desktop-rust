// Node builtins polyfill for Vite browser build.
// These are stubbed because real logic runs in Rust via bridge.
export const util = {
  promisify: (fn:any)=> (...args:any[])=> new Promise((_res,_rej)=> { throw new Error('util.promisify shim — execFile via Rust') }),
  inspect: (o:any)=> JSON.stringify(o),
  format: (...a:any[])=> a.join(' '),
  types: {},
}
export function promisify(fn:any){ return (...args:any[])=> new Promise((_,rej)=> rej(new Error('promisify shim — child_process via Rust')) ) }

export class Buffer extends Uint8Array {
  static alloc(n:number){ return new Buffer(n) }
  static from(s:any){ return Buffer.from(s?.toString()||'') }
  toString(){ return new TextDecoder().decode(this) }
}
export function createHash(){ return { update(){return this}, digest(){return ''} } }
export function randomUUID(){ return crypto.randomUUID() }
export function randomBytes(n:number){ return new Uint8Array(n) }

export class EventEmitter {
  on(_e:string,_f:any){ return this }
  once(_e:string,_f:any){ return this }
  emit(_e:string,..._a:any[]){ return false }
  removeListener(_e:string,_f:any){ return this }
}
export const assert = Object.assign(function(...a:any[]){}, { ok:()=>{}, strictEqual:()=>{}, deepEqual:()=>{} })

export class Readable { on(_e:string,_f:any){ return this } pipe(){ return this } }
export class Writable { write(_d:any,_cb?:any){ if(_cb)_cb() } end(){ } }
export class Stream { }

export const timers = { setTimeout, clearTimeout, setInterval, clearInterval, setImmediate: (fn:any)=>setTimeout(fn,0) }
export function setImmediate(fn:any){ return setTimeout(fn,0) }
export function clearImmediate(){}

export function parse(_s:string,_f?:any){ return {} }
export function stringify(_o:any){ return '' }
export function format(...a:any[]){ return a.join(' ') }
export function escape(s:string){ return s }
export function unescape(s:string){ return s }
