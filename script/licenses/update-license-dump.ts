import * as path from 'path'
import { promisify } from 'util'

import { licenseOverrides } from './license-overrides'

import _legalEagle from 'legal-eagle'
const legalEagle = promisify(_legalEagle)

import { getVersion } from '../../app/package-info'
import { readFile, writeFile } from 'fs/promises'

const assertValidLicensesIn = async (dir: string) => {
  const summary = await legalEagle({
    path: dir,
    // Make sure we pass a copy of the overrides to legal-eagle so that we don't
    // mutate the original object later when we `delete summary[key]`.
    // This is because legal-eagle returns the overrides object:
    // https://github.com/atom/legal-eagle/blob/b78443856a964f38c543c66b3b4feff5411c1167/src/legal-eagle.coffee#L11
    overrides: structuredClone(licenseOverrides),
    omitPermissive: true,
  })

  // Sourced from OSPOs license-policy
  const additionalAllowedLicenses = [
    '0BSD',
    'BlueOak-1.0.0',
    'Python-2.0',
    'MPL-2.0',
    'CC0-1.0',
  ]

  for (const key in summary) {
    const license = summary[key]
    if (additionalAllowedLicenses.includes(license.license)) {
      delete summary[key]
    }
  }

  if (Object.keys(summary).length > 0) {
    let licensesMessage = ''
    for (const key in summary) {
      const license = summary[key]
      licensesMessage += `${key} (${license.repository}): ${license.license}\n`
    }

    const overridesPath = path.join(__dirname, 'license-overrides.ts')

    const message = `The following dependencies have unknown or non-permissive licenses. Check it out and update ${overridesPath} if appropriate:\n${licensesMessage}`
    throw new Error(message)
  }
}

/** Collect the Desktop and dependency notices used by packaged renderers. */
export async function createLicenseDump(projectRoot: string) {
  const appRoot = path.join(projectRoot, 'app')

  await assertValidLicensesIn(projectRoot)
  await assertValidLicensesIn(appRoot)

  const summary = await legalEagle({
    path: appRoot,
    overrides: structuredClone(licenseOverrides),
  })

  // legal-eagle still chooses to ignore the LICENSE at the root
  // this injects the current license and pins the source URL before we
  // dump the JSON file to disk
  const licenseSource = path.join(projectRoot, 'LICENSE')
  const licenseText = await readFile(licenseSource, { encoding: 'utf-8' })
  const appVersion = getVersion()

  summary[`desktop@${appVersion}`] = {
    repository: 'https://github.com/desktop/desktop',
    license: 'MIT',
    source: `https://github.com/desktop/desktop/blob/release-${appVersion}/LICENSE`,
    sourceText: licenseText,
  }

  return summary
}

export async function updateLicenseDump(
  projectRoot: string,
  outRoot: string
): Promise<void> {
  const outPath = path.join(outRoot, 'static', 'licenses.json')
  const summary = await createLicenseDump(projectRoot)

  await writeFile(outPath, JSON.stringify(summary), { encoding: 'utf8' })
}
