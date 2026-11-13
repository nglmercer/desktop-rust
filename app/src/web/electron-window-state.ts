export function windowStateKeeper() {
  return {
    load: async () => ({ x: 0, y: 0, width: 1200, height: 800 }),
    save: async () => {},
  }
}
