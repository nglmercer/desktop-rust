import { describe, expect, it, vi } from 'vitest'
import { BackendClient, BackendError } from '../../src/backend/client'

describe('repository reveal cancellation', () => {
  it('does not treat an invalid response as a successful launch', async () => {
    const client: BackendClient = Object.create(BackendClient.prototype)
    Object.defineProperty(client, 'transport', { value: 'ws' })
    vi.spyOn(client, 'request').mockResolvedValue({})
    await expect(client.revealRegisteredRepository('/saved/repo')).rejects.toMatchObject({
      code: 'invalid_response', retryable: false,
      message: expect.stringContaining('may have opened'),
    })
  })

  it('reports an interrupted launch as uncertain and does not retry', async () => {
    const client: BackendClient = Object.create(BackendClient.prototype)
    Object.defineProperty(client, 'transport', { value: 'ws' })
    const controller = new AbortController()
    const request = vi.spyOn(client, 'request').mockImplementation(() =>
      new Promise((_, reject) => {
        controller.signal.addEventListener('abort', () => reject(new BackendError({
          code: 'cancelled', message: 'Interrupted', retryable: true,
        })), { once: true })
      })
    )
    const result = client.revealRegisteredRepository('/saved/repo', { signal: controller.signal })
    controller.abort()
    await expect(result).rejects.toMatchObject({
      code: 'cancelled', retryable: false,
      message: expect.stringContaining('may have opened'),
    })
    expect(request).toHaveBeenCalledTimes(1)
    expect(request).toHaveBeenCalledWith('reveal_repository', { repository_path: '/saved/repo' }, { signal: controller.signal })
  })
})
