import assert from 'node:assert'
import { it } from '../helpers/node-test-compat'
import { showNotification } from 'desktop-notifications'

it('does not report successful Windows submission before initialization', {
  skip: !__WIN32__,
}, async () => {
  assert.strictEqual(await showNotification('Title', 'Body'), null)
})
