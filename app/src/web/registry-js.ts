export class Registry {
  get() {
    return null
  }
  set(_k: string, _v: any) {}
}
export const HKEY = { LOCAL_MACHINE: 0, CURRENT_USER: 1 }
export const RegistryValueType = {
  String: 0,
  ExpandString: 1,
  Binary: 2,
  Int: 3,
  MultiString: 4,
}
export function enumerateValues(_p: string) {
  return [] as any
}
export function enumerateKeys(_p: string) {
  return [] as string[]
}
export function setValue(_p: string, _k: string, _v: any) {}
