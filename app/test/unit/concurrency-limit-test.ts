import assert from 'node:assert/strict'
import { describe, it } from 'vitest'
import { limitConcurrency } from '../../src/lib/concurrency-limit'

describe('limitConcurrency', () => {
  it('runs work in FIFO order without exceeding the limit', async () => {
    const limit = limitConcurrency(2)
    let active = 0
    let maximum = 0
    const events: string[] = []
    const work = Array.from({ length: 6 }, (_, index) =>
      limit(async () => {
        events.push(`start${index}`)
        active++
        maximum = Math.max(maximum, active)
        await new Promise(resolve => setTimeout(resolve, 1))
        active--
        events.push(`end${index}`)
        return index
      })
    )
    assert.deepEqual(await Promise.all(work), [0, 1, 2, 3, 4, 5])
    assert.equal(maximum, 2)
    assert.deepEqual(
      events.filter(event => event.startsWith('start')),
      ['start0', 'start1', 'start2', 'start3', 'start4', 'start5']
    )
  })

  it('continues queued work after a rejection', async () => {
    const limit = limitConcurrency(1)
    const failed = limit(() => {
      throw new Error('failed')
    })
    const completed = limit(() => 'completed')
    await assert.rejects(failed, /failed/)
    assert.equal(await completed, 'completed')
  })
})
