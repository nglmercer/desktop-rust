# Rust migration roadmap (Linux)

Scope: **Linux only.** Windows and macOS validation is deliberately deferred and
is not a gate here. This file turns the open removal gates in
[`MIGRATION.md`](MIGRATION.md) into ordered, checkable tasks. Update the
checkboxes and the baseline as work lands; re-measure with:

```sh
pnpm migration:status
```

## 1. Where we are (measured 2026-11-14)

`pnpm migration:status` on this checkout:

| Measure | Now | Done when |
|---|---|---|
| Fallback checks (`transportKind`) in the renderer | **231** in 68 files | 0 |
| Legacy Git runner calls (`await git(`) in `lib/git` | **148** in 43 files | 0 |
| Renderer files importing `dugite` | 27 | 0 |
| Renderer files importing `fs` | 53 | 0 (host operations only) |
| Renderer files importing `child_process` | 16 | 0 |
| Renderer files importing `os` / `crypto` / `keytar` | 10 / 8 / 1 | 0 |
| Unit test files still on the Node runner | **63** of 412 | 0 |
| Rust code (crates, non-test) | 89,093 lines | — |

Removed and verified on Linux: Electron, Webpack and esbuild (build, packages,
lockfiles, CI, docs), the Electron-shaped IPC shim, the native Node addon
dependencies. The Rust shell starts (`app_smoke` passes under Xvfb), the Vite
renderer mounts, and the full suite is green (338 Vitest files; 1353 Node tests
pass, 4 `todo`, 1 skipped).

Estimated completion, by workstream (judgement, not a measurement):

| Workstream | Est. |
|---|---|
| Remove Electron, Webpack, esbuild | 100% |
| Rust shell and host services on Linux | ~75% |
| Git operations served by Rust (renderer reaches them through typed ops) | ~55% |
| Renderer free of non-Rust paths (fallbacks, Node APIs) | ~25% |
| Tests exercising the Rust backend instead of fallbacks | ~15% |
| Assisted commit / Copilot / GitHub (`gh`) parity | ~50% |
| **Whole migration** | **~45–50%** |

Why the renderer figure is low: every `lib/git` module still carries a Node/
dugite fallback because the unit tests run with no Rust backend attached. Nothing
can be deleted until tests drive Rust (task R1), so R1 gates everything else.

## 2. Phases and tasks

Effort: S = a day or less, M = a few days, L = a week or more.

### R0 — Guardrails (S)
- [x] `pnpm migration:status` prints the baseline above (`script/migration-metrics.ts`).
- [x] `pnpm migration:check` fails CI if any number increases; `pnpm migration:baseline` lowers it.
- [ ] Record the numbers after each merged slice in the table above.

### R1 — Make tests drive the Rust backend (M, blocks R2–R5)
- [x] Shared harness `app/test/helpers/rust-backend.ts` (real `desktop-rs serve`, WebSocket client). Proven on `pathExists` / `directoryExists`.
- [x] `DESKTOP_TEST_RUST_BACKEND=1` runs every Vitest file against a real Rust server.
- [ ] Build `desktop-cli` before Vitest in the default test script so the harness never skips locally (today it skips when the binary is missing).
- [x] `useRustBackend()` routes `backend()` to the harness for a test file (and selects the repository each request is about).
- [ ] Add a reusable Rust-backed repository fixture (replaces `setupEmptyRepository`/`makeCommit` for tests that need a repository registered with the server).
- [ ] Decide server lifetime: one server per test file (simple) vs a shared worker (fast). Measure; target < 1 s overhead per file.
- [ ] Move the 63 Node-runner test files to Vitest (needed anyway so there is one runner). By folder: `stores` 21, `ui` 17, top level 15, `git` 10. Drop `script/ts-register.cjs` and `app/test/globals.cjs` when the last one moves.

### R2 — Retire fallbacks, module by module (L; the bulk of the work)

**How to measure it.** `DESKTOP_TEST_RUST_BACKEND=1 pnpm exec vitest run` runs
every Vitest file against a real Rust server (about 5 minutes). First
measurement: 332 of 396 files passed, 34 failed (198 tests). After the basis
wrappers and harness opt-outs: **339 of 397 pass, 28 fail (126 tests)**. Each failing file
is one of four kinds, and the fix for each kind is different:

| Kind | Meaning | Fix |
|---|---|---|
| Basis | The Rust operation takes the head/branch the app just read (`expected`) and refuses without it. | Wrap the call in the test with `currentBasis` / `mergeBasis` (`app/test/helpers/rust-backend.ts`). Mechanical; done for cherry-pick, rebase, revert, merge. |
| Contract | The Rust operation deliberately refuses what the old path allowed (continue with unstaged conflicts, pull without terminal output, custom rebase editors, unsupported history options). | Decide the contract; update the test, or add the capability in Rust. |
| Progress | Callers pass a progress callback; the Rust path emits none. | Stream progress events from the Rust operation. Blocks the cherry-pick and rebase progress tests. |
| Error shape | The Rust path fails differently on a non-repository or a symlinked `.gitignore` than the Node path. | Match the error text/type, or change the assertion. |

Worklist (failing files, with the first cause each hits):
- [x] `git/cherry-pick-test` basis, merge basis — 16 of 20 pass; remaining 4 are Progress ×3 and Contract ×1
- [x] `git/rebase/*`, `git/rebase-full-test` basis — 29 of 37 pass; remaining 8 are Contract ×4, Progress ×3, Error shape ×1
- [x] `git/revert-test`, `git/merge-test` basis — 10 of 11 pass; remaining 1 is Error shape (`abortMerge` with no merge)
- [x] `git/commit-test` basis: the test reviews the selection as the app does (`reviewSelectedCommit`); 25 of 27 pass with the Rust backend. Remaining 2 are Contract: committing conflicted files through `createCommit`, and the error text for `createMergeCommit` with nothing to commit. The Rust path returns the full object id and a real id for a first commit where the Node path returned an abbreviation and Git's `(root-commit)` marker; the test accepts both.
- [~] `git/rev-parse-test` (16 of 17 left): the harness now restarts the server with the test's Git environment (ownership check, tracing, home) when it changes between requests, which fixed one case; the rest fail because the Rust `trust` operation answers "cannot add Git directory exception" for the paths these tests add — investigate the Rust side
- [~] `git/remote-operations-test` (11 of 40 left): rebase/merge basis done; push needs its expected reference, and a remote literally named `--remote` is refused by Rust (Contract)
- [ ] `git/pull/*` (25): Contract / Basis
- [ ] `git/branch-test` (2): Basis — needs a reviewed branch tip
- [x] `git/branch-test`, `git/stash-test`, `git/checkout-test`, `git/gitignore-test` (Rust now names the symlinked root `.gitignore`), `git/description-test`, `write-default-readme-test`, `git/status-test`, `git/for-each-ref-test`: pass with the Rust backend on
- [ ] `git/remote-discovery-test` (4 left): Rust runs remote discovery with global/system config pinned to an empty file on purpose (`process.rs`), so the tests that expect global/system remotes and Git's empty-URL reset semantics need a decision: keep the isolation and change the tests' expectation under Rust, or read those scopes. Needs the user's call because the isolation is a security choice.
- [x] `git/worktree-move-test`: passes with the Rust backend on (the spy on the Node runner is skipped there)
- [ ] `git/revision-consumers-test` (3): leading-dash refs and ranges; not yet investigated
- [ ] `git/stash-test` (2), `git/remote-management-test` (2), `git/checkout-test` (2), `git/lfs-test` (2), `git/status-test`, `git/squash-test`, `git/reorder-test`, `git/for-each-ref-test`, `git/log-revision-exclusions-test`: Contract
- [ ] `pull-hook-callbacks-test`, `write-default-readme-test`: Contract
- [ ] `web-open-path-test`, `web-host-bridge-loader-test`, `script/*`: not backend work — they stub `window`, which the harness also does; make them opt out of the harness
- [ ] When the flag-on run is green, make it the default, then delete the fallbacks below.

Then, per module: delete the `transportKind` branch and the `git(` call, run
`pnpm migration:status` and lower the baseline with `pnpm migration:baseline`.
Order is lowest risk first. Counts are `legacy git calls / transport checks /
test files`.

**Wave 1 — small, mostly reads (S each)**
- [ ] `description` 0/2/2 · `init` 1/1/2 · `var` 1/1/1 · `revert` 1/1/1 · `rev-parse` 1/1/3 · `add` 1/0/2 · `clone` 1/0/2 · `stage` 0/1/7 · `checkout-index` 1/1/1 · `merge-tree` 0/1/1 · `diff-check` 1/0/2 · `gitignore` 0/3/1 · `refs` 1/2/4 · `pull` 1/1/4 · `push` 1/2/5 · `status` 1/3/4 · `update-index` 1/3/9

**Wave 2 — medium (S–M each)**
- [ ] `commit` 2/3/3 · `show` 1/4/5 · `reflog` 2/4/1 · `for-each-ref` 2/4/4 · `rev-list` 2/6/5 · `rm` 2/1/3 · `diff-index` 2/2/1 · `interpret-trailers` 2/2/3 · `fetch` 3/4/5 · `checkout` 4/3/5 · `reset` 4/3/5 · `tag` 3/4/2 · `merge` 4/4/6 · `cherry-pick` 4/5/4 · `submodule` 4/4/3 · `update-ref` 6/1/4 · `apply` 5/1/2

**Wave 3 — large or high-risk (M–L each)**
- [ ] `branch` 6/7/3 · `stash` 7/7/3 · `remote` 8/6/6 · `worktree` 5/7/4 · `rebase` 5/6/6 · `log` 3/8/8 · `lfs` 4/6/2
- [ ] `config` 5/11/18 — widest fan-out; do after the typed config reads cover every caller.
- [ ] `diff` 10/19/16 — largest; includes image/binary/large-file diffs and custom filters.
- [ ] `repository-operation` 4/3/59 — the operation fence used by 59 test files; do last, after the rest no longer need the async-context shim.

**Non-Git renderer areas with fallbacks**
- [ ] `lib/stores` (18 checks), `ui/clone-repository` (13), `ui/delete-branch` (10), `ui/add-repository` (8), `lib/custom-integration`, `lib/large-files`, `lib/copilot-conflict-context`, `ui/changes`, `ui/diff`, `ui/discard-changes`, `ui/dispatcher`.

### R3 — Assisted commit and Copilot in Rust (L)
- [ ] Replace the assisted-commit transaction that depends on Node async hooks (`repository-operation-context`) with a Rust transaction; then delete the async-hooks shim.
- [ ] Rust destination guard that does not depend on `remote.<name>.url=` reset semantics (works on Git 2.45 and 2.56); un-skip the five assisted-push suites on any Git.
- [ ] Fix the four "ignored missing alternate" cases (`todo`): decide whether a missing alternate should fail at status load or at push, implement once, un-`todo` them.
- [ ] List Copilot models and quotas through Rust operations (the Wry shell rejects unscoped `copilot_*` calls, so the JS SDK client cannot do this); remove `createClient` from the renderer store.
- [ ] Live-service validation of the commit-message flow (needs credentials; run manually on Linux).

### R4 — GitHub operations through `gh` (M–L)
- [ ] PR / issue / checks / workflow runs with enterprise accounts and clear auth and network errors.
- [ ] Remaining account/credential flows that still use `keytar` (1 file) → Rust credential store only.

### R5 — Remove Node API use from the renderer (M)
Follows R2; each item is a grep that must reach zero.
- [ ] `dugite` (27 files) → none.
- [ ] `fs` (53 files; 18 in `lib/git`) → host operations only.
- [ ] `child_process` (16), `os` (10), `crypto` (8) → host operations or Web APIs.
- [ ] Delete the matching `app/src/web/*` shims (`fs.ts`, `child_process.ts`, `dugite.ts`, `os.ts`, …).

### R6 — Delete legacy code and packages (M)
- [ ] Remove `dugite`, `desktop-trampoline` (Node module), `windows-argv-parser`, `desktop-notifications` vendor TypeScript, `printenvz` Node helper once their Rust replacements are the only path.
- [ ] Collapse duplicate crates: `desktop-core` (legacy shell core, 11.7k lines) into `core`, delete `native` (285 lines).
- [ ] Rename `host-bridge` IPC channels from the old Electron vocabulary only if the Rust side is changed in the same commit.
- [ ] `pnpm migration:status` reports 0 for every row in section 1.

### R7 — Linux validation (M)
- [ ] Real browser journeys (clone, commit, branch, stash, rebase, conflicts) against a large repository, with Unicode and byte-exact filenames.
- [ ] Command cancellation, reconnect and state consistency under load.
- [ ] Security review of the bridge (authentication, path confinement, credential handling).
- [ ] Package smoke: `pnpm package`, relocate, `start:prod`, drain on exit.
- [ ] Build and run the final tree with the legacy directories absent.

## 3. Known issues backlog (Linux)

- [ ] Finish splitting the renderer bundle (entry chunk about 2.4 MB after the first pass): `re2js` (make branch-rule matching async), the `crypto`/`readable-stream` polyfills, and the 800 KB of stores.

- [ ] `lfs-test` is skipped here: install `git-lfs` (CI installs it).
- [ ] This checkout sets `LOCAL_GIT_DIRECTORY` to Git 2.45.3; the runner builds a layout over the Git on PATH to run suites needing newer behavior (`script/test-capabilities.ts`). Retire this when R3's destination guard no longer needs it.
- [ ] Sign-in callback: an installed `github-desktop-rs.desktop` competes with the dev build; decide the dev handler policy.
- [ ] Sass `@import` and global colour functions: migrate the stylesheets, then drop the `silenceDeprecations` list in `vite.config.ts`.
- [ ] Run `pnpm start` on a real display and record any difference from the Xvfb run.

## 4. Definition of done

The migration is complete (on Linux) when all of these hold:

1. `pnpm migration:status` shows 0 fallback checks, 0 legacy Git calls and 0 Node-API imports in the renderer.
2. Every unit test runs on Vitest; the Node runner, `ts-register.cjs` and `globals.cjs` are gone.
3. The nine removal gates in `MIGRATION.md` are ticked with the evidence recorded there.
4. The full suite passes with no `todo` or capability-skipped suites other than documented external tools.
5. `pnpm start`, `pnpm build:prod` and `pnpm package` work from a clean checkout.
