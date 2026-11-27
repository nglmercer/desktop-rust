# Rust migration roadmap

Scope: Linux first. Windows and macOS validation remains required before any
release. `pnpm migration:status` is the live metric source;
`pnpm migration:check` must pass before committing migration slices.

## Current measured baseline

The enforced numeric baseline lives in `rust/migration-baseline.json`; use
`pnpm migration:status` for current values rather than copying them here.

| Measure | Current state | Done when |
| --- | --- | --- |
| Renderer `transportKind` fallback checks | Zero | 0 |
| Renderer-driven Git command sites | Zero | 0 |
| Renderer `dugite` / `os` / `crypto` / `keytar` imports | Zero | 0 |
| Renderer `fs` / `child_process` / `async_hooks` / `net` / `path` / `registry-js` imports | Nonzero | 0 |
| Test files importing the `node:test` compatibility API | Nonzero | 0 |

These measurements cover renderer consumers outside the implementation of the
compatibility shims themselves. Legacy host crates, credential/trampoline
machinery, test fixtures, and release infrastructure remain migration work.

## Ordered work

### R0 — Guardrails

- [x] Measure and enforce migration metrics.
- [ ] Keep qualitative migration documentation aligned after verified slices.

### R1 — Rust-backed tests

- [x] Run TypeScript tests through Vitest and the real Rust test backend.
- [ ] Add a reusable Rust-backed repository fixture.
- [ ] Decide per-file versus shared backend lifetime after measurement.

### R2 — Remaining runtime ownership

- [ ] Replace renderer hook-environment capture, `printenvz`, and process-proxy
      ownership with typed Rust operations.
- [ ] Move remaining trampoline credential/environment ownership into Rust.
- [ ] Resolve exact remote-configuration versus typed transport admission and
      finish the cross-Git destination guard.
- [ ] Stream live progress for rebase and cherry-pick.
- [ ] Remove compatibility shims only after their consumers are gone.

### R3 — Assisted commit and Copilot

- [ ] Replace the assisted-commit transaction's Node async-hooks ownership with
      a Rust transaction and delete the async-hooks shim.
- [ ] Keep missing-alternate behavior strict at push and non-fatal for status
      and protection.
- [ ] Move Copilot model/quota/session ownership into scoped Rust operations.
- [ ] Validate one live Rust-client Copilot turn and the packaged runtime layout
      (manual credential-gated check).

### R4 — GitHub and credentials

- [ ] Complete PR, issue, check, workflow-run, enterprise, pagination, and
      network-failure behavior through the Rust broker or a reviewed `gh` path.
- [ ] Replace remaining `keytar` shim credential flows with Rust credential storage.
- [ ] Migrate renderer HTTP/API callers to the Rust broker with cancellation,
      timeout, proxy, TLS, and rate-limit behavior.

### R5 — Renderer Node API removal

- [x] Reach zero measured renderer imports for `dugite`, `os`, `crypto`, and
      `keytar`.
- [ ] Reach zero measured renderer imports for `fs`, `child_process`,
      `async_hooks`, `net`, `path`, and `registry-js`.
- [ ] Delete each `app/src/web/*` shim after production consumers and dependent
      tests no longer use it.

### R6 — Legacy deletion

- [ ] Remove `dugite`, `desktop-trampoline`, vendored notification TypeScript,
      and `printenvz` when their Rust replacements are the only paths.
- [ ] Merge useful `desktop-shell-core-legacy` services into `core` and retire
      `native` as an independent host.
- [ ] Remove duplicated TypeScript persistence/domain projections once Rust is
      authoritative.
- [ ] Build and test the tree with legacy directories absent.

### R7 — Linux validation

- [ ] Exercise real browser journeys for clone, commit, branch, stash, rebase,
      and conflicts on large repositories, including Unicode and byte-exact names.
- [ ] Validate cancellation, reconnects, and state consistency under load.
- [ ] Review bridge authentication, path confinement, credential handling,
      request budgets, and sync-IPC exposure.
- [ ] Package, relocate, launch, exercise, and cleanly shut down the result.

## Platform and release gates

- [ ] Validate Tao/Wry, credentials, protocol registration, editors, terminals,
      and shell integration on real macOS and Windows systems.
- [ ] Validate low-resource/Termux headless builds without GUI dependencies.
- [ ] Produce signed/notarized installers and a signed update channel controlled
      by this fork.
- [ ] Complete license and notice inventory validation.

## Known issues

- The renderer entry chunk remains large.
- `lfs-test` requires `git-lfs`; CI installs it but local environments may not.
- Test Git version capability is normalized by `script/test-capabilities.ts`.
- An installed `github-desktop-rs.desktop` can compete with development builds
  for sign-in callbacks.
- Sass `@import` and global color functions still emit controlled deprecations.
- Cross-file backend integration tests can interfere through shared process and
  network resources; isolate failures before treating them as product regressions.

## Definition of done on Linux

1. Migration metrics reach zero for renderer fallback, renderer-driven Git
   command, compatibility-import, and `node:test` compatibility rows.
2. The full validation gate in `MIGRATION.md` passes.
3. Browser, cancellation, reconnect, large-repository, and package smoke evidence exists.
4. Legacy shims, hosts, packages, and helpers have either been removed or have
   an explicit typed host-only justification.
5. `pnpm start`, `pnpm build:prod`, and `pnpm package` work from a clean checkout.
