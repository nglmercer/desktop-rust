// Use Node-compatible stream constructors for browserified dependencies.
// @ts-expect-error -- aliased to stream-browserify in vite.config.ts
import streamBrowserify from 'desktop-stream-impl'

export const {
  Readable,
  Writable,
  Duplex,
  PassThrough,
  Transform,
  pipeline,
  finished,
} = streamBrowserify

export const Stream = streamBrowserify
// Retain the previous shim's compatibility names for consumers that import
// these from `stream` instead of the filesystem module.
export function ReadStream() {}
export function WriteStream() {}
export default streamBrowserify
