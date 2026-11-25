// `registry-js` for the webview and Node tests. The Windows registry is not
// reachable from here, so every read is empty. The types follow the package's
// public API so the Windows integrations keep compiling.
export enum HKEY {
  HKEY_CLASSES_ROOT = 'HKEY_CLASSES_ROOT',
  HKEY_CURRENT_USER = 'HKEY_CURRENT_USER',
  HKEY_LOCAL_MACHINE = 'HKEY_LOCAL_MACHINE',
  HKEY_CURRENT_CONFIG = 'HKEY_CURRENT_CONFIG',
  HKEY_USERS = 'HKEY_USERS',
}

export enum RegistryValueType {
  REG_NONE = 'REG_NONE',
  REG_SZ = 'REG_SZ',
  REG_EXPAND_SZ = 'REG_EXPAND_SZ',
  REG_BINARY = 'REG_BINARY',
  REG_DWORD = 'REG_DWORD',
  REG_DWORD_LITTLE_ENDIAN = 'REG_DWORD_LITTLE_ENDIAN',
  REG_DWORD_BIG_ENDIAN = 'REG_DWORD_BIG_ENDIAN',
  REG_LINK = 'REG_LINK',
  REG_MULTI_SZ = 'REG_MULTI_SZ',
  REG_QWORD = 'REG_QWORD',
}

export interface RegistryStringEntry {
  readonly name: string
  readonly type: RegistryValueType.REG_SZ | RegistryValueType.REG_EXPAND_SZ
  readonly data: string
}

export interface RegistryValue {
  readonly name: string
  readonly type: RegistryValueType
  readonly data: string
}

export function enumerateValues(
  _hive: HKEY,
  _key: string
): ReadonlyArray<RegistryValue> {
  return []
}

export function enumerateKeys(
  _hive: HKEY,
  _key: string
): ReadonlyArray<string> {
  return []
}

export function setValue(
  _hive: HKEY,
  _key: string,
  _name: string,
  _type: RegistryValueType,
  _data: string
): boolean {
  return false
}
