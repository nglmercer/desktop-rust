---
name: testing
description: >-
  Instructions for writing and maintaining tests in GitHub Desktop. Covers unit
  tests, UI component tests, and ad-hoc E2E tests. Use this skill when
  implementing features or bugfixes to write relevant tests, update existing
  tests, run the full suite to check for regressions, and produce screenshots
  and videos for Pull Request documentation.
---

# Testing in GitHub Desktop

This document describes the three tiers of tests in GitHub Desktop, how to run
them, and the patterns you should follow when writing new tests or updating
existing ones as part of a feature or bugfix.

## Overview

| Tier | Purpose | Location | Runner |
|------|---------|----------|--------|
| Unit / integration (non-UI) | Pure logic, stores, models, git operations | `app/test/unit/` | `node:test` via `npm test` |
| UI component | React components rendered in JSDOM | `app/test/unit/ui/` | `node:test` + React Testing Library via `npm test` |
| Browser / Wry integration | Real renderer in Chromium or the Wry shell | `rust/tests/` | Vitest |

### When to use each tier

- **Unit / integration**: new or changed logic in `app/src/lib/`, `app/src/models/`, git operations, store behavior, utility functions, IPC contracts.
- **UI component**: new or changed React components, dialog behavior, banners, toolbar items, list rendering.
- **Ad-hoc E2E**: only for **temporary** validation of a feature or bugfix across the full app. E2E tests you write are meant to be run locally and to capture screenshots/video for the PR, **not** to be merged into the permanent smoke suite.

---

## Running Tests

```bash
# All unit and UI tests
npm test

# A specific test file
npm test -- app/test/unit/my-feature-test.ts

# All tests in a directory (recursive)
npm test -- app/test/unit/ui

```

The test runner (`script/test.mts`) discovers files matching
`-test.(ts|tsx|js|jsx|mts|mjs)` recursively in `app/test/unit/` by default, or
in the paths you pass.

---

## Test Verification Workflow

After implementing any change you **must** run the full unit test suite:

```bash
npm test
```

If any tests fail:

1. Determine whether the failure is a **regression** (a bug you introduced) or
   an **expected behavior change** (your change intentionally altered the
   behavior).
2. If it is a regression, fix the code.
3. If it is an expected change, update the test assertions so they reflect the
   new correct behavior.
4. Re-run the suite until everything passes.

Then verify linting:

```bash
npm run lint
```

If lint errors are reported and you want to auto-fix them:

```bash
npm run lint:fix
```

> **Note:** `npm run lint:fix` rewrites files across the repository (Prettier +
> ESLint `--fix`). Only run it when you intend to apply those edits — do not
> use it as a read-only check.

---

## Bug-First Testing

When fixing a bug:

1. **Write a failing test first** that reproduces the bug.
2. Verify the test fails.
3. Apply the fix.
4. Verify the test now passes.

This proves the fix works and protects against regressions.

---

## Unit / Integration Tests (Non-UI)

### File conventions

- Location: `app/test/unit/`, mirroring the source tree
  (e.g. `app/src/lib/git/clone.ts` → `app/test/unit/git/clone-test.ts`).
- File name: `*-test.ts`.
- Extension: `.ts` (use `.tsx` only when the file contains JSX).

### Imports

```ts
import { describe, it, beforeEach } from 'node:test'
import assert from 'node:assert'
```

Use `node:assert` for all assertions — never Jest or Chai matchers.

### Test structure

Synchronous tests are fine for pure logic:

```ts
describe('MyFeature', () => {
  it('does something useful', () => {
    const result = myFunction('input')
    assert.equal(result, 'expected')
  })
})
```

Use `async` when the test or its helpers need it. Pass the test context `t`
when using helpers that register cleanup via `t.after()`:

```ts
it('creates a repo', async t => {
  const repo = await setupEmptyRepository(t)
  // repo's temp directory is cleaned up automatically after the test
})
```

### Assertion patterns

| Pattern | Use |
|---------|-----|
| `assert.equal(a, b)` | Abstract equality (`==`) — use when coercion is intentional |
| `assert.strictEqual(a, b)` | Strict equality (`===`) — preferred; catches type mismatches |
| `assert.deepEqual(a, b)` | Deep structural equality |
| `assert.notEqual(a, b)` | Abstract inequality (`!=`) |
| `assert.notStrictEqual(a, b)` | Strict inequality (`!==`) |
| `assert.ok(value)` | Truthy check |
| `assert.rejects(asyncFn, /pattern/)` | Async rejection with message |
| `assert.throws(fn, /pattern/)` | Sync throw |

> **`assert.equal` vs `assert.strictEqual`**: `assert.equal(a, b)` uses the `==` operator
> (abstract equality), so `assert.equal(42, '42')` passes. `assert.strictEqual(a, b)` uses
> `===`, so it also checks that types match. **Prefer `assert.strictEqual`** in most cases
> to avoid silent type-coercion surprises. Use `assert.equal` only when you explicitly
> want coercion semantics.

### Existing helpers — reuse them

| Helper file | Key exports | Purpose |
|-------------|------------|---------|
| `app/test/helpers/repositories.ts` | `setupEmptyRepository(t)`, `setupFixtureRepository(t, name)`, `setupConflictedRepo(t)` | Create temporary git repos with automatic cleanup |
| `app/test/helpers/repository-scaffolding.ts` | `makeCommit()`, `createBranch()`, `switchTo()`, `cloneRepository()` | Build git state (commits, branches) |
| `app/test/helpers/temp.ts` | `createTempDirectory(t)` | Temporary directory with auto-cleanup via `t.after()` |
| `app/test/helpers/mock-api.ts` | `createMockAPI(overrides)`, `createMockAPIRepository()`, `createMockAPIIdentity()` | Proxy-based mock API — rejects unmocked methods to prevent real HTTP requests |
| `app/test/helpers/mock-ipc.ts` | `MockIPC` | Records `send()`/`invoke()` calls, simulates main→renderer messages via `emit()` |
| `app/test/helpers/app-store-test-harness.ts` | `createTestStores()`, `createTestAccountsStore()`, `createTestRepositoriesStore()` | Factory functions for wired-up test store instances backed by in-memory storage |
| `app/test/helpers/test-stats-store.ts` | `TestStatsStore` | In-memory stats store for verifying metric increments |
| `app/test/helpers/stores/` | `InMemoryStore`, `AsyncInMemoryStore` | Key-value stores for testing code that depends on persistent storage |
| `app/test/helpers/databases/` | `TestRepositoriesDatabase`, `TestIssuesDatabase`, etc. | Dexie database wrappers with `reset()` for cleanup |
| `app/test/helpers/git.ts` | `getTipOrError()`, `getRefOrError()`, `getBranchOrError()` | Safe git object accessors for tests |
| `app/test/helpers/random-data.ts` | `generateString()` | Random hex strings using crypto |

### Patterns to follow

**Factory functions for dependencies** — create stores, databases, and API
instances through dedicated factory functions, not raw constructors:

```ts
const stores = createTestStores()
const api = createMockAPI({
  fetchRepository: async () => createMockAPIRepository(),
})
```

**Promise wrappers with timeouts** for callback-based async APIs:

```ts
async function waitForResult(store, ...args): Promise<Result> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error('Timed out')),
      5_000
    )
    store.getResult(...args, result => {
      clearTimeout(timeout)
      resolve(result)
    })
  })
}
```

**State machine testing** — verify store transitions by calling methods and
asserting intermediate states:

```ts
signInStore.beginDotComSignIn()
const state = signInStore.getState()
assert.equal(state?.kind, SignInStep.Authentication)
```

**Compile-time contract verification** — use TypeScript's type system to catch
missing cases at compile time (see `ipc-contract-test.ts` for example):

```ts
type AssertExactUnion<TExpected, TActual> = [
  Exclude<TExpected, TActual>,
  Exclude<TActual, TExpected>,
] extends [never, never]
  ? true
  : never
```

---

## UI Component Tests

### File conventions

- Location: `app/test/unit/ui/`.
- File name: `*-test.tsx` (must be `.tsx` for JSX).

### Critical import rule

**Always** import render utilities from the project's wrapper module:

```tsx
import { render, fireEvent, screen, waitFor, within } from '../../helpers/ui/render'
```

**Never** import directly from `@testing-library/react`. The wrapper module
(`app/test/helpers/ui/render.tsx`) imports `app/test/helpers/ui/setup.ts` as a
side-effect, which:

1. Polyfills `ResizeObserver` (not available in JSDOM).
2. Aligns `globalThis.Event`/`CustomEvent` with the jsdom window versions.
3. Registers an `afterEach(cleanup)` hook so the DOM is cleaned between tests.

Skipping this import will cause test failures or leaks.

### Rendering and querying

```tsx
import assert from 'node:assert'
import { describe, it } from 'node:test'
import * as React from 'react'
import { render, screen, fireEvent } from '../../helpers/ui/render'

describe('MyComponent', () => {
  it('renders a button and responds to clicks', () => {
    let clicked = 0
    render(<MyComponent onClick={() => clicked++} />)

    const button = screen.getByRole('button', { name: 'Submit' })
    assert.ok(button)

    fireEvent.click(button)
    assert.equal(clicked, 1)
  })
})
```

**Querying elements:**

| Method | Use |
|--------|-----|
| `screen.getByRole('button', { name: 'X' })` | Accessible role + name (preferred) |
| `screen.getByText('Hello')` | Visible text content |
| `screen.getByTestId('my-id')` | `data-testid` attribute |
| `view.container.querySelector('.css-class')` | CSS selector on the render container |
| `screen.queryByRole(...)` | Returns `null` instead of throwing (for absence checks) |

**Assertions** use `node:assert`, not Jest matchers:

```tsx
assert.notEqual(view.container.querySelector('.my-class'), null)
assert.equal(screen.queryByRole('button', { name: 'Gone' }), null)
```

### Re-rendering

```tsx
const view = render(<MyComponent visible={true} />)
// ... assert initial state ...
view.rerender(<MyComponent visible={false} />)
// ... assert updated state ...
```

### Callback verification

Capture callbacks in local variables and assert after interaction:

```tsx
let dismissed = 0
render(<Banner onDismissed={() => dismissed++} />)
fireEvent.click(screen.getByRole('button', { name: 'Dismiss this message' }))
assert.equal(dismissed, 1)
```

### Timer mocking

For components with timeouts (banners, auto-dismiss, debounce):

```tsx
import { afterEach, beforeEach, describe, it } from 'node:test'
import {
  advanceTimersBy,
  enableTestTimers,
  resetTestTimers,
} from '../../helpers/ui/timers'

describe('auto-dismissing banner', () => {
  beforeEach(() => enableTestTimers(['setTimeout']))
  afterEach(() => resetTestTimers())

  it('dismisses after timeout', () => {
    let dismissed = 0
    render(<Banner timeout={500} onDismissed={() => dismissed++} />)

    advanceTimersBy(500)
    assert.equal(dismissed, 1)
  })
})
```

### Clipboard testing

Register `restore()` in `afterEach` so the mock is always torn down even when
an assertion throws:

```tsx
import { afterEach, it } from 'node:test'
import { captureClipboardWrites } from '../../helpers/ui/host-ipc'

describe('CopyButton', () => {
  let restore: () => void
  let writes: string[]

  afterEach(() => restore?.())

  it('copies text to clipboard', () => {
    ;({ writes, restore } = captureClipboardWrites())
    render(<CopyButton text="hello" />)
    fireEvent.click(screen.getByRole('button'))
    assert.deepEqual(writes, ['hello'])
  })
})
```

Calling `restore()` inline at the end of the test body is **not** safe — if
any assertion before it throws, the global `clipboard.writeText` mock stays
patched and will silently contaminate subsequent tests.

### ESLint note

The `react/jsx-no-bind` rule is disabled for test files, so inline arrow
functions in JSX are fine in tests.

---

## Test Helpers Reference

### Global test environment (`app/test/globals.cjs`)

This file is loaded automatically by the test runner. It:

- Imports `fake-indexeddb/auto` and `global-jsdom/register` for browser API
  simulation.
- Defines Vite build-time constants (`__DEV__`, `__APP_NAME__`, etc.).
- Mocks the host bridge module (host IPC and shell).
- Removes `MessageChannel`/`MessagePort`/`BroadcastChannel` to prevent test
  hangs (React 16 + Dexie cleanup issue).

You do **not** need to set up any of this manually — it runs before every test
file.

### Environment variables (`.test.env`)

Loaded automatically by the test runner. Sets `GIT_AUTHOR_NAME`,
`GIT_COMMITTER_NAME`, etc. so git operations produce deterministic results.

---

## Checklist

When you are done implementing a feature or bugfix, verify:

- [ ] Wrote unit tests for new or changed logic.
- [ ] Wrote UI component tests for new or changed React components.
- [ ] Existing tests updated if behavior intentionally changed.
- [ ] `npm test` passes with no failures.
- [ ] `npm run lint` passes (run `npm run lint:fix` to auto-fix if needed).
- [ ] (If applicable) Ran ad-hoc E2E test and captured screenshots/video.
- [ ] (If applicable) Attached screenshots and video to the PR.
