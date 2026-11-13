declare module 'path-browserify' {
  const path: typeof import('node:path').posix
  export = path
}
