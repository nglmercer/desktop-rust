type ClassDictionary = Record<string, unknown>
type ClassArray = ReadonlyArray<ClassValue>
export type ClassValue =
  | string
  | number
  | null
  | false
  | undefined
  | ClassDictionary
  | ClassArray

const hasOwn = Object.prototype.hasOwnProperty
const objectToString = Object.prototype.toString

export function classNames(...values: ReadonlyArray<ClassValue>): string {
  let classes = ''
  for (const value of values) {
    if (value) {
      classes = appendClass(classes, parseValue(value))
    }
  }
  return classes
}

function parseValue(value: ClassValue): string | number {
  if (typeof value === 'string' || typeof value === 'number') {
    return value
  }
  if (typeof value !== 'object' || value === null) {
    return ''
  }
  if (Array.isArray(value)) {
    return classNames(...(value as ClassArray))
  }
  if (
    value.toString !== objectToString &&
    !value.toString.toString().includes('[native code]')
  ) {
    return value.toString()
  }
  const dictionary = value as ClassDictionary
  let classes = ''
  for (const key in dictionary) {
    if (hasOwn.call(dictionary, key) && dictionary[key]) {
      classes = appendClass(classes, key)
    }
  }
  return classes
}

function appendClass(value: string, newClass: string | number): string {
  if (!newClass) {
    return value
  }
  return value ? `${value} ${newClass}` : `${newClass}`
}
