# Rust migration roadmap (Linux)

Scope: **Linux first.** Windows and macOS validation remains necessary before
release, but the ordered removal work below is gated on Linux evidence.
`pnpm migration:status` re-measures the baseline; `pnpm migration:check` fails
if it regresses.

## 1. Current baseline (measured 2026-11-25)

| Measure | Now | Done when |
| --- | --- | --- |
| Fallback checks (`transportKind`) in the renderer | 3 in 3 files | 0 |
| Legacy Git runner calls (`await git(`) in `lib/git` | 28 in 10 files | 0 |
| Renderer files importing `dugite` | 17 | 0 |
| Renderer files importing `fs` | 17 | 0 (host operations only) |
| Renderer files importing `child_process` | 7 | 0 |
| Renderer files importing `os` / `crypto` / `keytar` | 0 / 0 / 0 | 0 |
| Unit test files on the Node runner | 0 of 406 | 0 |
| Rust code (crates, non-test) | 90,267 lines | — |

Completed foundations: Electron, Webpack and esbuild are out of the active build;
the Tao/Wry shell starts under Linux/Xvfb; the renderer builds with Vite; all
TypeScript tests run through Vitest; oxlint, TypeScript 7 and Biome form the fast
verification gate.

## 2. Ordered work

Effort: S = a day or less, M = a few days, L = a week or more.

### R0 — Guardrails (S)

- [x] `pnpm migration:status` prints the baseline.
- [x] `pnpm migration:check` fails CI if any number increases;
      `pnpm migration:baseline` lowers it after verified removals.
- [ ] Refresh the numbers in this file after each merged slice.

### R1 — Make tests exercise the Rust backend (M)

- [x] Shared harness `app/test/helpers/rust-backend.ts` starts real
      `desktop-rs serve` instances and routes `backend()` through the browser
      WebSocket client.
- [x] `script/test.mts` builds `desktop-cli` on Linux and enables
      `DESKTOP_TEST_RUST_BACKEND=1` by default when the build succeeds.
- [x] Move all former Node-runner suites to Vitest. Legacy `node:test` imports
      use the compatibility alias; there is no separate Node test run.
- [ ] Add a reusable Rust-backed repository fixture replacing ad hoc
      `setupEmptyRepository`/`makeCommit` server registration.
- [ ] Decide server lifetime: one server per test file versus a shared worker.
      Measure before optimizing; target less than 1 s overhead per file.

### R2 — Retire fallbacks module by module (L; bulk of the work)

For each module: prove behavior against the Rust backend, resolve contract or
progress differences, delete the `transportKind` branch and legacy `git(` call,
run the focused tests, then lower the baseline.

Classify failures before changing code:

| Kind | Meaning | Typical fix |
| --- | --- | --- |
| Basis | Rust requires the head/branch/operation basis the app just read. | Pass `currentBasis`/`mergeBasis` or bind the operation correctly. |
| Contract | Rust deliberately refuses behavior the old path allowed. | Decide the contract; change the caller/test or implement the capability. |
| Progress | Callers expect live progress but Rust emits none. | Stream progress events from Rust. |
| Error shape | Rust fails with a different type/message. | Align the contract or update assertions deliberately. |

Priority guidance:

1. Low-risk read-only modules and already-routed modules first.
2. Medium Git mutation modules once basis and progress contracts exist.
3. High-fan-out modules (`config`, `diff`, `repository-operation`) last.
4. Non-Git fallback clusters in `lib/stores`, clone/delete/add repository UI,
   changes/diff/discard UI, custom integration, large files and Copilot conflict
   context after their Git dependencies are gone.

Known contract decisions still open:

- Pull in repositories with submodules currently returns `unsupported_submodules`.
- Free-form history filters such as `--grep` remain Node-only; the app's typed
  first-parent/remotes queries should not keep that fallback.
- Rust remote discovery intentionally isolates global/system configuration;
  decide whether tests and product behavior should preserve that isolation.
- Rebase and cherry-pick do not stream live progress yet.

### R3 — Assisted commit and Copilot in Rust (L)

- [ ] Replace the assisted-commit transaction depending on Node async hooks
      (`repository-operation-context`) with a Rust transaction; then delete the
      async-hooks shim.
- [ ] Finish the destination guard across supported Git versions without
      relying on `remote.<name>.url=` reset output quirks.
- [ ] Keep missing-alternate behavior strict at the push stage and non-fatal
      for status/run protection.
- [ ] List Copilot models and quotas through scoped Rust operations; remove
      direct SDK client creation from the renderer store.
- [ ] Validate one live Rust-client Copilot turn and the distributed runtime
      layout (requires credentials; manual Linux gate).

### R4 — GitHub operations through Rust/`gh` (M–L)

- [ ] PRs, issues, checks and workflow runs with enterprise accounts and clear
      authentication/network failures.
- [ ] Migrate remaining account/credential flows that use the `keytar` shim to
      Rust credential storage only.
- [ ] Migrate renderer HTTP/API callers, including Copilot API access, to the
      Rust broker with cancellation, timeout, proxy, TLS, pagination and
      rate-limit behavior.

### R5 — Remove Node API use from the renderer (M)

Follows R2. Each grep must reach zero or be explicitly host-only:

- [ ] `dugite` (currently 17 files)
- [ ] `fs` (currently 17 files)
- [ ] `child_process` (7), `os` (0), `crypto` (0), `keytar` (0)
- [ ] Delete the matching `app/src/web/*` shims only after their imports are gone.
- [ ] Remove `app/test/globals.cjs` once the import-cycle test no longer needs
      a Node subprocess with the old module-loader environment.

### R6 — Delete legacy code and packages (M)

- [ ] Remove `dugite`, `desktop-trampoline` as a Node module,
      `windows-argv-parser`, `desktop-notifications` vendor TypeScript and the
      `printenvz` helper when Rust replacements are the only path.
- [ ] Collapse `rust/crates/desktop-core` (`desktop-shell-core-legacy`) into
      `rust/crates/core` and retire `rust/crates/native` as a host.
- [x] Migrate release version handling away from the npm CLI and retire
      `app/package-lock.json`.
- [ ] `pnpm migration:status` reports zero for every migration row.

### R7 — Linux validation (M)

- [ ] Real browser journeys (clone, commit, branch, stash, rebase, conflicts)
      against a large repository, including Unicode and byte-exact filenames.
- [ ] Cancellation, reconnect and state consistency under load.
- [ ] Bridge security review: authentication, path confinement, credential
      handling, request budgets and sync-IPC allowlist.
- [ ] Package smoke: `pnpm package`, relocate the result, run it outside the
      checkout and verify clean shutdown.
- [ ] Build and run the final tree with legacy directories absent.

## 3. Platform and release gates after Linux

- [ ] Validate the Tao/Wry shell, credentials, protocol registration, terminal
      and editor integrations on macOS and Windows.
- [ ] Produce signed/notarized installers and a signed update channel controlled
      by this fork. The current package is a development package.
- [ ] Verify WebView2 bootstrap behavior on Windows and WebKitGTK/WKWebView
      behavior on Linux/macOS.
- [ ] Validate low-resource/Termux headless builds without GUI dependencies.

## 4. Known issues backlog

- [ ] Finish splitting the renderer bundle: `re2js`, crypto/readable-stream
      polyfills and large stores still contribute to the entry chunk.
- [ ] `lfs-test` requires `git-lfs`; CI installs it, local environments may not.
- [ ] This checkout can set `LOCAL_GIT_DIRECTORY` to an older Git. The runner
      builds a capable layout over PATH Git when needed
      (`script/test-capabilities.ts`); retire that workaround when version-sensitive
      behavior is fully contract-tested.
- [ ] Sign-in callback: an installed `github-desktop-rs.desktop` can compete
      with the dev build; decide the development handler policy.
- [ ] Sass `@import` and global color functions: migrate stylesheets, then drop
      the `silenceDeprecations` list in `vite.config.ts`.
- [ ] Run `pnpm start` on a real display and compare with the Xvfb smoke run.

## 5. Definition of done

The migration is complete on Linux when all of these hold:

1. `pnpm migration:status` shows zero fallback checks, zero legacy Git calls and
   zero non-host Node API imports in the renderer.
2. Every TypeScript test runs on Vitest without a separate Node runner or suite
   routing file.
3. The removal gates in `MIGRATION.md` are ticked with reproducible evidence.
4. The full suite passes with no undocumented `todo` or capability-skipped tests.
5. `pnpm start`, `pnpm build:prod` and `pnpm package` work from a clean checkout.
6. The fast gate (`pnpm lint`, `pnpm check:types`, `pnpm compile:script`) passes
   in seconds, not minutes.
