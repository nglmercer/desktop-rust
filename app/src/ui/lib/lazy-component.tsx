import * as React from 'react'

/**
 * A component loaded on first render, in its own chunk. It is typed as the
 * component it wraps so callers use it exactly like a static import.
 *
 * The placeholder is an empty element rather than nothing so a transition
 * wrapped around it (modal popups) always has a DOM node to animate; the real
 * component replaces it when its chunk arrives.
 */
export function lazyComponent<C extends React.ComponentType<any>>(
  load: () => Promise<{ readonly default: C }>
): C {
  const Lazy = React.lazy(load)
  const LazyComponent = (props: React.ComponentProps<C>) => (
    <React.Suspense fallback={<div className="lazy-placeholder" />}>
      <Lazy {...(props as React.ComponentProps<C>)} />
    </React.Suspense>
  )
  LazyComponent.displayName = 'LazyComponent'
  return LazyComponent as unknown as C
}
