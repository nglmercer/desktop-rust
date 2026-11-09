export class GitError extends Error { readonly gitError:any; readonly path:string; constructor(msg:string, info:any){ super(msg); this.gitError=info } }
export class ExecError extends Error {}
export class DugiteError extends Error {}
export class GitBinary extends Error {}
export type IGitResult = any
export type IGitExecutionOptions = any
export type IGitSpawnOptions = any
export function exec(){ return Promise.reject(new Error('dugite shim')) }
export function spawn(){ throw new Error('dugite spawn shim') }
export function resolveGitBinary(){ return '' }
export function getGitPath(){ return '' }
export function parseGitConfig(){ return {} }
export function parseError(){ return { stdout:'', stderr:'', code:1 } }
export function parseBadConfigValueErrorInfo(){ return {} }
export const ERROR = {}
export function resolveExecutable(){ return '' }
export default { exec, spawn, GitError, ExecError, resolveGitBinary }
