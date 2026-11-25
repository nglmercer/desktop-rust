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
export default streamBrowserify
