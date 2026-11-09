export function createHook(_opts?:any){ return { enable:()=>{}, disable:()=>{}, run: (fn:any)=>fn(), executionAsyncId:()=>0 } }
export function executionAsyncId(){ return 0 }
export function getDefaultTriggerAsyncId(){ return 0 }
export function createAsyncResource(_opts?:any){ return { run: (fn:any)=>fn(), emitDestroy:()=>{} } }
export const AsyncLocalStorage = {}
