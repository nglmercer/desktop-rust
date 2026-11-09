export class EventEmitter { on(_e:string,_f:any){ return this } once(_e:string,_f:any){ return this } emit(_e:string,..._a:any[]){ return false } removeListener(_e:string,_f:any){ return this } addListener(_e:string,_f:any){ return this } }
export default EventEmitter
