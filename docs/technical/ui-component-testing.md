# UI component testing with Testing Library

This document describes the supported path for DOM-based UI component tests in
GitHub Desktop.

## Runner

All TypeScript tests run under Vitest. Some older files still import the
`node:test` surface, but `vitest.config.ts` aliases those imports to
`app/test/helpers/node-test-compat.ts`; they are not executed by Node's test
runner. Write new tests directly against Vitest.

## File locations

- Put component tests under `app/test/unit/ui`.
- Keep shared UI helpers under `app/test/helpers/ui`.
- Use `-test.tsx` for React component tests.

## Running UI tests

```sh
pnpm test -- app/test/unit/ui/tab-bar-test.tsx
pnpm test -- app/test/unit/ui
pnpm test
```

`pnpm test` discovers tests through `script/test.mts` and invokes Vitest. On
Linux it builds `desktop-cli` first when the Rust backend harness is needed;
set `DESKTOP_TEST_RUST_BACKEND=0` only for a deliberately backend-less run.

## Preferred imports

Always import render utilities from the project wrapper:

```tsx
import { render, fireEvent, screen, waitFor, within } from '../../helpers/ui/render'
```

Do not import directly from `@testing-library/react`. The wrapper installs the
jsdom environment fixes, `ResizeObserver` polyfill and automatic cleanup.

Use `node:assert/strict` for assertions and `describe`/`it` from `vitest` for
new files. Query by accessible role and visible text before using CSS selectors.
