/** Quote a Git environment configuration parameter, without invoking a shell. */
export function quoteGitConfigParameter(parameter: string): string {
  return `'${parameter.replace(/'/g, "'\\''")}'`
}
