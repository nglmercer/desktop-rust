# Rust migration roadmap (Linux)

Scope: **Linux only.** Windows and macOS validation is deliberately deferred and
is not a gate here. This file turns the open removal gates in
[`MIGRATION.md`](MIGRATION.md) into ordered, checkable tasks. Update the
checkboxes and the baseline as work lands; re-measure with:

```sh
pnpm migration:status
```

## 1. Where we are (measured 2026-11-17)

`pnpm migration:status` on this checkout:

| Measure | Now | Done when |
|---|---|---|
| Fallback checks (`transportKind`) in the renderer | **202** in 59 files | 0 |
| Legacy Git runner calls (`await git(`) in `lib/git` | **127** in 32 files | 0 |
| Renderer files importing `dugite` | 24 | 0 |
| Renderer files importing `fs` | 51 | 0 (host operations only) |
| Renderer files importing `child_process` | 16 | 0 |
| Renderer files importing `os` / `crypto` / `keytar` | 10 / 8 / 1 | 0 |
| Unit test files still on the Node runner | **0** of 413 | 0 |
| Rust code (crates, non-test) | 89,197 lines | — |

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
- [x] Build `desktop-cli` before Vitest in the default test script (`script/test.mts`). A failed build warns and the harness skips; the warning is the only signal, so keep the build step green.
- [x] `useRustBackend()` routes `backend()` to the harness for a test file (and selects the repository each request is about).
- [ ] Add a reusable Rust-backed repository fixture (replaces `setupEmptyRepository`/`makeCommit` for tests that need a repository registered with the server).
- [ ] Decide server lifetime: one server per test file (simple) vs a shared worker (fast). Measure; target < 1 s overhead per file.
- [x] Move the 63 Node-runner test files to Vitest (needed anyway so there is one runner). 63 → 0 remain. The last six were converted by replacing `mock.module` with `vi.doMock`, routing CJS-patched `fs/promises` methods through `vi.mock`-backed spies (the `tailer-test` pattern), and reinstalling method mocks in `beforeEach`. `assisted-commit-test`, `assisted-commit-history-test` and `assisted-commit-push-boundary-test` were all brought onto the Rust harness (context-menu mock, `GIT_ATTR_SOURCE` forwarding, and a per-test Node-path backend stub for the 12 fault-injection cases). Only `commit-message-test` still opts out; it renders with fabricated repository paths and tests UI behaviour only. `script/ts-register.cjs` is still used by build/release scripts, so it remains; `app/test/globals.cjs` is no longer used by the test runner.

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
- [x] `git/rev-parse-test`: passes with the Rust backend on. The earlier "cannot add Git directory exception" failures were a harness bug (a restarted backend session was not wrapped, so no repository was selected), not a Rust fault. `git/remote-management-test` was the same bug plus a vanished-remote contract that `updateRemoteHEAD` now tolerates.
- [x] `git/merge-test` and `git/commit-test` pass with the Rust backend on (they accept the Rust error text; a conflict resolution is committed with `createMergeCommit`, as the app does).
- [x] **The Vitest suites now run against the Rust backend by default on Linux** (`pnpm test` builds `desktop-cli` and sets `DESKTOP_TEST_RUST_BACKEND=1`; `DESKTOP_TEST_RUST_BACKEND=0` opts out, but tests of code whose fallback is already deleted then fail). Full default run: Vitest 339 files / 2951 tests pass, `node:test` 1353 pass / 0 fail / 4 todo.
- [x] Wave 1 fallbacks retired: `description` (2 checks), `stage` (1) and `merge-tree` (1). Their functional tests pass with `DESKTOP_TEST_RUST_BACKEND=1` and the dedicated `rust-*-routing-test` files assert the Rust path is used exclusively, so the Node/`dugite` branches and their imports were deleted. `rust-git-description-test.ts` was updated to drop the now-removed `transport: 'none'` fallback case. Metrics: transport checks 228→224, `dugite` 27→26, `fs` 53→52; baseline lowered. Full `pnpm test` green: Vitest 393 files / 3936 tests pass (11 files / 32 tests skipped), `node:test` 369 pass / 0 fail. A second slice retired `gitignore` (3 checks), `revert` (1), and `init` (1): transport checks 224→219, legacy Git calls 145→143, `fs` 52→51; `rust-git-init-test.ts` dropped its `transport: 'none'` case like the description test. Full `pnpm test` green: Vitest 401 files / 4303 tests pass. A third slice retired `checkout-index` (1 check, 1 legacy Git call) after confirming `assisted-commit-push-boundary-test` still passes; transport checks 219→218, legacy Git calls 143→142. A fourth slice retired `update-index` (3 checks, 1 legacy Git call) — no caller passes `add`/`remove`/`replace: false`, so the Rust operation always applies; transport checks 218→215, legacy Git calls 142→141. A fifth slice retired `rm` (1 check, 2 legacy Git calls — `unstageAllFiles` now goes through Rust and the dead `removeConflictedFile` helper was deleted, with `checkout-test` using the `rawGit` helper directly); transport checks 215→214, legacy Git calls 141→139. A sixth slice retired `diff-index` (1 check, 2 legacy Git calls) with the same verification; transport checks 214→213, legacy Git calls 139→137. A seventh slice retired `rev-list` (3 checks, 2 legacy Git calls) — `getAheadBehind`, `getCommitsInRange` and `doMergeCommitsExistAfterCommit` now use only Rust reads; transport checks 213→210, legacy Git calls 137→135, `dugite` 26→25. `reflog` was first blocked by the backend-less `node:test` suites, then by the harness-opt-out suites. Both gates are now cleared: `assisted-commit-push-boundary-test` runs on the Rust harness with 12 fault-injection cases temporarily routed to a Node-path backend stub (the module-patch and `GIT_OBJECT_DIRECTORY` faults are invisible to the separate Rust process), and that stub supplies the two incidental `repositoryRendererReflog` reads the app store performs. `reflog`'s fallback is deleted; transport checks 210→208, legacy Git calls 135→133. `refs` followed once the stub also supplied `repositorySymbolicRef`; transport checks 208→207, legacy Git calls 133→132. `tag` followed with `repositoryRendererTagRefs` in the stub; transport checks 207→204, legacy Git calls 132→129. `for-each-ref` followed with `repositoryBranches`; transport checks 204→202, legacy Git calls 129→127, `dugite` 25→24.
- Kept on the Git/Node path under the Rust backend, each with a stated reason: pull in a repository with submodules (Rust answers `unsupported_submodules`; blocks deleting the pull fallback), two LFS tests when the system Git has no git-lfs (environment), `pull-hook-callbacks-test` (it mocks the legacy runner), and free-form history filters such as `--grep` (the app uses only `--first-parent` and `--not --remotes`).
- First fallbacks deleted (`rename` and `delete` branch): transport checks 231→229, legacy Git calls 148→146. `createBranch` keeps its fallback because two `node:test` suites (`repository-operation-test`) exercise it without a backend: the `node:test` files are the gate for most remaining deletions, so moving them to Vitest (which has the Rust backend) comes first. `app/test/helpers/node-test-compat.ts` (aliased as `node:test` under Vitest) lets a suite move without rewriting its mocks; 63 → 32 files remain on `node:test` (see `pnpm migration:status`): the ones using `mock.module`, CJS patching of built-in modules (`tailer-test`), fake timers that differ under Vitest (`account-token-lifecycle-test`), and the assisted-commit suites that need the Rust destination guard or hit real Rust/legacy behavior differences (partial-selection commits).
- Stripping the guarded Node fallbacks across `app/src` (a mechanical transform) was tried and reverted: it broke the `node:test` assisted-commit suites (hook interception, push lifecycle, reader publication, run/hooks), which still run without the Rust backend. Those suites must move to Vitest (or gain a backend) before their fallbacks can go. Do not retry the bulk strip until then.
- Trial conversion of the 12 assisted-commit `node:test` suites to Vitest (`compat` layer): 6 suites are skipped entirely because they patch Node's built-in `fs`/`dugite` module objects in a `before` hook (CJS patching; `push-boundary` alone has 114 tests, `reader-publication` 22, `run-hooks` 10, `hooks` 16, `push-hooks` 5, `selected-fence` 2, `snapshot` 3). They must be rewritten to drive the Rust backend and real Git instead of patching modules. 6 run but fail on real behavior: `git/assisted-commit-test` (17 of 122), `git/assisted-commit-cleanup-test` (17), `stores/assisted-commit-cleanup-test` (11), `stores/assisted-commit-push-test` (15 of 59), `stores/assisted-commit-push-lifecycle-test` (7 of 16). Those are R3 work, not conversion work.
- Destination pin (R3, `app/src/lib/git/assisted-commit-push.ts`): the guard compared the raw `git remote get-url --push --all` output with the reviewed URL. Git 2.56 prints a URL reset as a blank line, and Git 2.45.3 (the version the tests and dugite use) repeats a `pushInsteadOf` destination. The guard now compares the distinct URLs after the last reset with the reviewed destination. That fixed the `pushInsteadOf`, reset and upstream cases; `stores/assisted-commit-push-test` on Vitest goes from 15 failing to 13 (the remaining causes are listed in the R3 section). The node:test destination, push and assisted-commit suites still pass (73 and 122).
- Remaining push-test failures on Vitest (backend on): the post-guard causes are (a) a legacy `git push origin <sha>:refs/heads/<branch>` run by the assisted push under a `GIT_CONFIG` shadow file fails with `fatal: no path specified` on Vitest but passes on `node:test` (the shadow-config test: `uses native upstream configuration…`, and `does not inherit mirror…`), so the Vitest environment changes how the test's shadow `GIT_CONFIG` applies; (b) credential rotation between commits does not reach `pushed`; (c) post-push refresh and tracking cases return `push-error`. Each needs its own diagnosis; none is the destination guard.
- Full migration attempt (all 32 `node:test` files moved to Vitest in one pass): 30 failed (99 tests: `mock.module` users, CJS-patched built-in modules, and Rust-backend behaviour differences). Only 2 passed and moved: `git/assisted-commit-push-destination-test` and `stores/assisted-commit-push-lifecycle-test`. The other 30 stay on `node:test` and run without the Rust backend. Migrating them needs their mocks rewritten and their behaviour fixes (listed above), not a bulk rename. Full suite after this split: Vitest 369 files / 3514 tests pass; `node:test` 792 pass / 0 fail.
- Rebase and cherry-pick report their steps after the native call returns instead of while it runs (the Rust operations do not stream progress yet); live progress streaming is still Rust work.
- [~] `git/remote-operations-test` (11 of 40 left): rebase/merge basis done; push needs its expected reference, and a remote literally named `--remote` is refused by Rust (Contract)
- [ ] `git/pull/*` (25): Contract / Basis
- [ ] `git/branch-test` (2): Basis — needs a reviewed branch tip
- [x] `git/branch-test`, `git/stash-test`, `git/checkout-test`, `git/gitignore-test` (Rust now names the symlinked root `.gitignore`), `git/description-test`, `write-default-readme-test`, `git/status-test`, `git/for-each-ref-test`: pass with the Rust backend on
- [ ] `git/remote-discovery-test` (4 left): Rust runs remote discovery with global/system config pinned to an empty file on purpose (`process.rs`), so the tests that expect global/system remotes and Git's empty-URL reset semantics need a decision: keep the isolation and change the tests' expectation under Rust, or read those scopes. Needs the user's call because the isolation is a security choice.
- [x] `git/worktree-move-test`: passes with the Rust backend on (the spy on the Node runner is skipped there)
- [x] `git/revision-consumers-test`: passes with the Rust backend on (free-form `--grep` is Node-only; the app uses only `--first-parent` and `--not --remotes`)
- [ ] `git/stash-test` (2), `git/remote-management-test` (2), `git/checkout-test` (2), `git/lfs-test` (2), `git/status-test`, `git/squash-test`, `git/reorder-test`, `git/for-each-ref-test`, `git/log-revision-exclusions-test`: Contract
- [ ] `pull-hook-callbacks-test`, `write-default-readme-test`: Contract
- [ ] `web-open-path-test`, `web-host-bridge-loader-test`, `script/*`: not backend work — they stub `window`, which the harness also does; make them opt out of the harness
- [ ] When the flag-on run is green, make it the default, then delete the fallbacks below.

Then, per module: delete the `transportKind` branch and the `git(` call, run
`pnpm migration:status` and lower the baseline with `pnpm migration:baseline`.
Order is lowest risk first. Counts are `legacy git calls / transport checks /
test files`.

**Wave 1 — small, mostly reads (S each)**
- [ ] `description` 0/0/2 · `init` 0/0/2 · `var` 1/1/1 · `revert` 0/0/1 · `rev-parse` 1/1/3 · `add` 1/0/2 · `clone` 1/0/2 · `stage` 0/0/7 · `checkout-index` 0/0/1 · `merge-tree` 0/0/1 · `diff-check` 1/0/2 · `gitignore` 0/0/1 · `refs` 0/1/4 · `pull` 1/1/4 · `push` 1/2/5 · `status` 1/3/4 · `update-index` 0/0/9

**Wave 2 — medium (S–M each)**
- [ ] `commit` 2/3/3 · `show` 1/4/5 · `reflog` 0/2/1 · `for-each-ref` 0/2/4 · `rev-list` 0/3/5 · `rm` 0/0/3 · `diff-index` 0/1/1 · `interpret-trailers` 2/2/3 · `fetch` 3/4/5 · `checkout` 4/3/5 · `reset` 4/3/5 · `tag` 0/1/2 · `merge` 4/4/6 · `cherry-pick` 4/5/4 · `submodule` 4/4/3 · `update-ref` 6/1/4 · `apply` 5/1/2

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
- [x] The four "ignored missing alternate" cases pass and are no longer `todo`: a missing alternate does not fail status load or run protection (neither reads the alternate); the push stage re-reads the resources strictly and reports it as a `push-error` with `attempted: false`.
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

### Bridge selection contract (blocks the app-store and harness races)

Facts checked in the code, so the next change starts from them:

- The server keeps one selected repository per session (`state.repo` in
  `rust/crates/bridge/src/lib.rs`). `repository_binding` returns a `root_id`
  only when the requested path is the selected repository.
- A bound read (`ApplicationOperation::Repository`) checks `repo.id() == expected`
  while it holds the selection lock, and holds the lock during execution, so bound
  operations already run one at a time.
- The window that breaks is between a client's binding and its read: another
  request can select a different repository in between, and the read fails with
  "Selected repository does not match". `app-store-add-accounts` (one test) and
  the harness hit this when a background fetch on one repository overlaps a
  request on another.
- Path-based operations (`Repository { path, ... }` variants) open or reuse the
  repository per request under the same lock, so they have no binding window.

Decision taken: option 1. A bound read now carries `repository_path` (the
server resolves it for that request under the selection lock, with the expected id
checked when given). Option 2 (client-side serialization) was tried and broke seven
websocket tests that need concurrency, so it was reverted. Options considered:
1. Route bound reads through the path-based variants (atomic; server change for
   every bound method the renderer uses).
2. Serialize binding and read per client (client change; no server change; the
   server already serializes execution, so no extra waiting for bound operations).

### Node test runner exception

Still on `node:test`, with the reason each one does not pass on Vitest yet. Each
file leaves this list when it passes on Vitest. Nothing is skipped to hide a
failure; all of them are green on `node:test`.

Classified outside the Rust harness on purpose: `ui/commit-message-test.tsx`
(80 of 80 pass on Vitest). It renders with fabricated repository paths to test
UI behaviour; Git behaviour is covered by the Git suites, which do run on the
backend. Opted out in `app/test/vitest-setup.ts`.

Fixed in the test helper, not in Rust: the compat `mock.fn` recorded a call
before running it, so `callCount()` inside an implementation included the current
call, unlike `node:test`. It now records after the call, as node does. This made
`stores/assisted-commit-run-test` pass on Vitest (39 of 39).

Verified state (full `pnpm test`, exit 0): Vitest 401 files pass / 11 skipped,
4303 tests pass / 32 skipped. There is no `node:test` run at all any more.

All 63 former `node:test` files are now Vitest suites. Conversions in this
slice: `app-store-accounts` (15), `assisted-commit-push-hooks` (5),
`git/assisted-commit-cleanup` (17), `git/assisted-commit` (122),
`stores/assisted-commit-cleanup` (11), `stores/assisted-commit-push` (59) and
`stores/assisted-commit-push-boundary` (114). `mock.module` became `vi.doMock`;
CJS-patched `fs/promises` methods (mkdtemp/writeFile/rm/lstat/rename/open/
readFile/unlink) became `vi.mock`-backed spies that delegate to the real
functions and reset in `beforeEach`; the accounts suite also reinstalls its
method mocks in `beforeEach` because the Vitest compat tracker restores
module-level mocks after every test. The previously unconfirmed `fatal: no path
specified` push-hooks hypothesis is resolved — all 5 tests pass with the real
hook proxies once the module mocks are converted.

`assisted-commit-history-test` and `assisted-commit-test` now run against the
Rust backend: the former mocks `showBrowserContextMenu` (the active path when a
transport is attached) in addition to `hostIpc.invoke`, and the harness now
forwards `GIT_ATTR_SOURCE` to the server so the Rust diff honours the attribute
source the fixture sets (122/122 pass). `assisted-commit-push-boundary-test`
now runs on the harness too: its 12 fault-injection cases route to a per-test
Node-path backend stub, the other 102 use Rust. The only suite still opted out
is `commit-message-test` (fabricated repository paths, UI behaviour only).
Previously resolved:
`stores/assisted-commit-selection-test` passes on Vitest 24/24; a file changed
after certification is refused, not committed from stale certified lines.
