export function setImmediate(fn:any){ return setTimeout(fn,0) }
export function clearImmediate(){}
export function setTimeout(...a:any[]){ const [fn,...rest]=a; return setTimeout(...a as any) }
export function setInterval(){ return 0 as any }
export function clearTimeout(id:any){ globalThis.clearTimeout(id) }
export function clearInterval(id:any){ globalThis.clearInterval(id) }
