export function pipeline(...a:any[]){ return Promise.reject(new Error('pipeline shim')) }
export function finished(){ return Promise.resolve() }
