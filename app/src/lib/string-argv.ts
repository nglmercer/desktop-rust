const argumentPattern = new RegExp(
  String.raw`([^\s'"]([^\s'"]*(['"])([^\3]*?)\3)+[^\s'"]*)|[^\s'"]+|(['"])([^\5]*?)\5`,
  'gi'
)

export function parseArgsStringToArgv(value: string): Array<string> {
  const result: Array<string> = []
  argumentPattern.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = argumentPattern.exec(value)) !== null) {
    result.push(firstString(match[1], match[6], match[0]))
  }
  return result
}

function firstString(...values: ReadonlyArray<string | undefined>): string {
  for (const value of values) {
    if (typeof value === 'string') {
      return value
    }
  }
  return ''
}
