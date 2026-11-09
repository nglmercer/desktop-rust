export function createHash(_a:string){ return { update(){return this}, digest(){return ''} } }
export function randomUUID(){ return crypto.randomUUID() }
export function randomBytes(n:number){ return new Uint8Array(n) }
