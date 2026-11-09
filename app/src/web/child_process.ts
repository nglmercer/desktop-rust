export function execFile(_f:string,_args?:any,_opts?:any,_cb?:any){ const cb = _cb||_opts; const err = new Error('child_process shim — use Rust bridge'); if(cb) cb(err, '', ''); throw err }
export function exec(_c:string,_cb?:any){ const err=new Error('child_process.exec shim'); if(_cb)_cb(err,'',''); throw err }
export function spawn(){ throw new Error('child_process.spawn shim') }
