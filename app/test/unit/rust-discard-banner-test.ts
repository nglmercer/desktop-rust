import { describe, expect, it, vi } from 'vitest'
import { BannerType } from '../../src/models/banner'
import { renderBanner } from '../../src/ui/banners/render-banner'
import type { Dispatcher } from '../../src/ui/dispatcher'

describe('selected discard undo banner', () => {
  it('presents the undo action', () => {
    const onUndo = vi.fn()
    const element = renderBanner(
      { type: BannerType.DiscardedSelection, undoId: 'test-id', onUndo },
      {} as Dispatcher,
      vi.fn()
    )
    expect(element.props.children).toBe('Selected changes discarded.')
    element.props.onUndo()
    expect(onUndo).toHaveBeenCalledOnce()
  })
})
