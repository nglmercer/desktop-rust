// Minimal fs shim for browser: nothing on disk, all via Rust bridge.
export function readFileSync(_p:string,_o?:any){ throw new Error('fs.readFileSync not available in web — use Rust bridge') }
export function writeFileSync(_p:string,_d:any){ throw new Error('fs.writeFileSync not available in web') }
export function existsSync(_p:string){ return false }
export function mkdirSync(_p:string,_o?:any){ }
export function readdirSync(_p:string){ return [] as string[] }
export function statSync(_p:string){ throw new Error('fs.statSync not available in web') }
export function unlinkSync(_p:string){ }
export function lstatSync(_p:string){ throw new Error('fs.lstatSync shim') }
export function opendirSync(_p:string){ throw new Error('opendirSync shim') }
export function realpathSync(_p:string){ return _p }
export function readlinkSync(_p:string){ return '' }
export function symlinkSync(_a:string,_b:string){ }
export function readFile(_p:string,_cb?:any){ if(_cb) _cb(new Error('fs.readFile shim')) }
export function writeFile(_p:string,_d:any,_cb?:any){ if(_cb) _cb(new Error('fs.writeFile shim')) }
export const promises = {
  readFile: async()=>{ throw new Error('fs/promises.readFile shim') },
  writeFile: async()=>{ throw new Error('fs/promises.writeFile shim')},
}
export function createReadStream(){ throw new Error('fs.createReadStream shim') }
export function createWriteStream(){ throw new Error('fs.createWriteStream shim') }
export const constants = { F_OK: 0, W_OK: 1, X_OK: 2, R_OK: 4 }
export class Stats {}
export class WriteStream {}
export class ReadStream {}
export class BigIntStats {}
export class Dirent {}
export function chmodSync(){}
export function copyFileSync(){}
export function mkdtempSync(){ return '' }
export function renameSync(){}
export function rmSync(){}
export function mkdirSync2(){}
export default { readFileSync, existsSync, mkdirSync, readdirSync, statSync, lstatSync, realpathSync, readlinkSync, symlinkSync, constants, Stats, WriteStream }
export function openSync(){ return 0 }
export function closeSync(){}
export function fdatasyncSync(){}
export function fsyncSync(){}
export function writeSync(){ return 0 }
export function readSync(){ return 0 }
export function fstatSync(){ return { size:0, isDirectory:()=>false, isFile:()=>true, mtime: new Date(), mode: 0o644, ino: 0, dev: 0, nlink:0, rdev:0, atime:new Date(), ctime:new Date() } }
export function fchmodSync(){}
export function fchownSync(){}
export function fdatasync(){}
export function fsync(){}
export function truncateSync(){}
export function statSync2(){}
export function ftruncateSync(){}
export function fchown(){}
export function watch(){ return { close:()=>{} } }
