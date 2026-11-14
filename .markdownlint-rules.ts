const markdownlintGitHub: unknown = require('@github/markdownlint-github')

if (
  markdownlintGitHub === null ||
  typeof markdownlintGitHub !== 'object' ||
  !Array.isArray(Reflect.get(markdownlintGitHub, 'default'))
) {
  throw new Error('Unable to load the GitHub Markdown lint rules')
}

module.exports = Reflect.get(markdownlintGitHub, 'default')
