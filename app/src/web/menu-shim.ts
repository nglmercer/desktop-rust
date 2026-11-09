export type MenuEvent = string
export type TestMenuEvent = string
export function isTestMenuEvent(_e:string): boolean { return false }
export const Menu = { buildFromTemplate: () => {} } as any
export const MenuItem = class {}
export function ensureItemIds(_i:any){ return _i }
export function buildDefaultMenu(){ return null as any }
export function buildContextMenu(){ return null as any }
export function getAllMenuItems(){ return [] as any }
