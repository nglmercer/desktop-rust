import { formatJsonFiles } from './format-json'

const paths = process.argv.slice(2)
if (paths.length === 0) {
  throw new Error('Specify at least one JSON file to format')
}
formatJsonFiles(paths)
