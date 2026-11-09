export class Readable { on(_e:string,_f:any){ return this } pipe(){ return this } }
export class Writable { write(_d:any,_cb?:any){ if(_cb)_cb() } end(){ } }
export class Duplex extends Readable {}
export class PassThrough extends Duplex { write(_d:any,_cb?:any){ if(_cb)_cb() } }
export class Transform extends Duplex {}
export function ReadStream(){}
export function WriteStream(){}
export function pipeline(...a:any[]){ return Promise.reject(new Error('pipeline shim')) }
