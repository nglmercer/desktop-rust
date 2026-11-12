import assert from 'node:assert'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { describe, it } from 'vitest'

import { generateUsageDataExample } from '../helpers/usage-data'
import { buildStatsPayload } from '../../src/lib/stats/stats-payload'
import type { StatsPayload } from '../../src/lib/stats/stats-store'

describe('usage data example', () => {
  it('matches the request body sent by StatsStore', async () => {
    const examplePath = resolve('docs/process/usage-data.json')
    const checkedInExample = await readFile(examplePath, 'utf8')
    const generatedExample = generateUsageDataExample()
    const expected = `${JSON.stringify(generatedExample, null, 2)}\n`

    assert.strictEqual(
      checkedInExample.replaceAll('\r\n', '\n'),
      expected,
      'Usage data example is stale. Run `npm run generate-example-usage-data`.'
    )
  }, 30_000)

  it('formats ping dimensions and rounds usage timing measures', () => {
    assert.deepStrictEqual(
      buildStatsPayload({
        eventType: 'ping',
        optIn: true,
        previousOptInValue: null,
      }),
      {
        events: [
          {
            app: 'desktop',
            event_type: 'ping',
            dimensions: { optIn: 'true', previousOptInValue: 'null' },
          },
        ],
      }
    )
    const usage = buildStatsPayload({
      eventType: 'usage',
      useExternalCredentialHelper: undefined,
      mainReadyTime: 1.5,
      loadTime: 2.4,
      rendererReadyTime: 3.6,
      commits: 4,
    } as unknown as StatsPayload)
    assert.strictEqual(
      usage.events[0].dimensions.useExternalCredentialHelper,
      'null'
    )
    assert.deepStrictEqual(usage.events[0].measures, {
      commits: 4,
      mainReadyTime: 2,
      loadTime: 2,
      rendererReadyTime: 4,
    })
  })
})
