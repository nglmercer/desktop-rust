import { cancelImmediate, scheduleImmediate } from './immediate'

type ImmediateCallback = (...args: unknown[]) => void

type ImmediateGlobals = {
  setImmediate?: unknown
  clearImmediate?: unknown
}

const scope = globalThis as unknown as ImmediateGlobals

// Dependencies using the Node entry point of vscode-jsonrpc expect these
// process globals even when every callback is browser-side. Provide the narrow
// compatible surface without pulling in the Node timers polyfill.
if (typeof scope.setImmediate !== 'function') {
  scope.setImmediate = (callback: ImmediateCallback, ...args: unknown[]) =>
    scheduleImmediate(callback, ...args)
}
if (typeof scope.clearImmediate !== 'function') {
  scope.clearImmediate = (handle: number) => cancelImmediate(handle)
}
