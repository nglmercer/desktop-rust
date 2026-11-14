import { describe, expect, it, vi } from 'vitest'
import { BackendClient } from '../../src/backend/client'

function clientWithResponse(value: unknown) {
  const client: BackendClient = Object.create(BackendClient.prototype)
  Object.defineProperty(client, 'transport', { value: 'ws' })
  const request = vi.fn().mockResolvedValue(value)
  const close = vi.fn()
  Object.defineProperty(client, 'changelogRequests', { value: 0, writable: true })
  Object.defineProperty(client, 'wsChangelog', { value: { request, close }, writable: true })
  return { client, request, close }
}

describe('Rust release changelog client', () => {
  it('passes only the channel and count to the typed backend operation', async () => {
    const release = { name: 'Release', notes: ['[Fixed] Example'], pub_date: '2025-01-01', version: '3.0.0' }
    const { client, request, close } = clientWithResponse([release])
    await expect(client.releaseChangelog('beta', 250)).resolves.toEqual([release])
    expect(request).toHaveBeenCalledWith('release_changelog', { channel: 'beta', limit: 250 }, { deadlineMs: 40_000 })
    expect(close).toHaveBeenCalledOnce()
  })

  it.each([
    null,
    {},
    [{ name: 'Release', notes: 'not-an-array', pub_date: '2025-01-01', version: '3.0.0' }],
    [{ name: 'Release', notes: [null], pub_date: '2025-01-01', version: '3.0.0' }],
  ])('rejects an invalid response', async value => {
    const { client } = clientWithResponse(value)
    await expect(client.releaseChangelog(null)).rejects.toMatchObject({ code: 'invalid_response' })
  })

  it.each([-1, 1.5, NaN, 251])('rejects an invalid limit without making a bridge request', async limit => {
    const { client, request } = clientWithResponse([])
    await expect(client.releaseChangelog(null, limit)).rejects.toMatchObject({ code: 'invalid_request' })
    expect(request).not.toHaveBeenCalled()
  })

  it('keeps the shared auxiliary connection until all concurrent requests settle', async () => {
    const { client, request, close } = clientWithResponse([])
    let finishFirst!: (value: unknown) => void
    let finishSecond!: (value: unknown) => void
    request.mockImplementationOnce(() => new Promise(resolve => { finishFirst = resolve }))
    request.mockImplementationOnce(() => new Promise(resolve => { finishSecond = resolve }))
    const first = client.releaseChangelog(null)
    const second = client.releaseChangelog('beta')
    finishFirst([])
    await expect(first).resolves.toEqual([])
    expect(close).not.toHaveBeenCalled()
    finishSecond([])
    await expect(second).resolves.toEqual([])
    expect(close).toHaveBeenCalledOnce()
  })

  it('closes the auxiliary connection when a request fails', async () => {
    const { client, request, close } = clientWithResponse([])
    request.mockRejectedValue(new Error('offline'))
    await expect(client.releaseChangelog(null)).rejects.toThrow('offline')
    expect(close).toHaveBeenCalledOnce()
  })
})
