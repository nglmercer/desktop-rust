export const access = async()=>{ throw new Error('fs.promises.access shim — pathExists will be bridged via ipc') }
export const readFile = async()=>{ throw new Error('fs/promises.readFile shim')}
export const writeFile = async()=>{ throw new Error('fs/promises.writeFile shim')}
export const stat = async()=>{ throw new Error('fs/promises.stat shim')}
export const lstat = async()=>{ throw new Error('fs/promises.lstat shim')}
export const readdir = async()=>{ throw new Error('fs/promises.readdir shim')}
export const mkdir = async()=>{ }
export const unlink = async()=>{ }
export const rename = async()=>{ throw new Error('fs.promises.rename shim')}
export const realpath = async(p:string)=>{ return p }
export const open = async()=>{ throw new Error('fs.promises.open shim')}
export const copyFile = async()=>{ throw new Error('fs.promises.copyFile shim')}
export const rm = async()=>{ }
export const appendFile = async()=>{ throw new Error('fs.promises.appendFile shim')}
export const truncate = async()=>{ }
export const FileHandle = class {}
export const lstatSync = ()=>{ throw new Error('lstatSync shim')}
export const realpathSync = { native: (p:string)=>p }
export const constants = { F_OK: 0, W_OK: 1, X_OK: 2, R_OK: 4 }
export const opendir = async()=>{ throw new Error('opendir shim') }

export const cp = async()=>{ throw new Error('cp shim') }
export const mkdtemp = async(_p:string)=> '/tmp/vite-mkdtemp'
export const readlink = async(_p:string)=> ''
export const symlink = async()=>{ }
export const constants2 = {}
// Duplicate missing still needed: ensure mkdtemp exported
export const chmod = async()=>{ throw new Error('shim chmod') }
