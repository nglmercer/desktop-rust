export async function du(_p:string){ return 0 }
export async function trash(_p:string){ throw new Error('trash shim — use ipc move-to-trash') }
