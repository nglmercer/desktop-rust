import { describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({ blob: vi.fn() }))
vi.mock('../../src/lib/git/show', () => ({ getBlobContents: mocks.blob }))
vi.mock('../../src/lib/custom-integration', () => ({}))
vi.mock('keytar', () => ({ default: {} }))

import { getBlobImage } from '../../src/lib/git/diff'

describe('committed image data', () => {
  it('passes only the blob bytes to the DDS decoder, excluding surrounding buffer data', async () => {
    const allocation = Buffer.from([10, 11, 0x44, 0x44, 0x53, 0x20, 12, 13])
    const blob = allocation.subarray(2, 6)
    mocks.blob.mockResolvedValue(blob)
    const repository = { path: '/repo' } as Repository
    const image = await getBlobImage(repository, 'image.DDS', 'HEAD')
    expect(mocks.blob).toHaveBeenCalledWith(repository, 'HEAD', 'image.DDS')
    expect([...new Uint8Array(image.rawContents)]).toEqual([...blob])
    expect(image.bytes).toBe(blob.length)
    expect(image.contents).toBe(blob.toString('base64'))
    expect(image.mediaType).toBe('image/vnd-ms.dds')
  })
})
