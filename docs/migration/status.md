# Estado de la migración — desktop-rust

> Fase 0 — inventariado el 2026-11-10 sobre base `ca6acec28f` (`development`).
> Cada fila: consumidor → backend actual → destino → contrato → estado → evidencia → pendiente.
> Estados: **implementado** (backend definitivo) · **integrado** (conectado al consumidor) · **probado** (recorrido verificado) · **bloqueado** (entorno o credencial pendiente).

## Fase 0 — base verificable

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Helpers de prueba `BINARY`/`WebSocket` | `lifecycle`, `cancellation`, `signals`, `management`, `repository_accounts`, `lfs_operations`, `github`, `editor_conflicts` | `rust/tests/smoke.py` eliminado en `2b1530be11` | `rust/tests/support/bridge.py` | `BINARY: Path`, `WebSocket{send,receive,close}`, `read_exact`, `run` | **implementado** · **integrado** · **probado** | `python3 rust/tests/lifecycle.py`, `cancellation.py`, `signals.py`, `management.py`, `github.py`, `editor_conflicts.py` PASS en esta sesión | `lfs_operations.py` bloqueado sin `git-lfs` local; en CI GitHub `git lfs version` pasa |
| Workflow headless/GUI separados, submódulos, Vite antes de embed | CI | [`.github/workflows/rust.yml`](../../.github/workflows/rust.yml) con `submodules: recursive`, pnpm antes de `cargo build`, `web-*` vía `script/test.mjs` | mismo | pasos: checkout → submódulos → pnpm install → `vite:build` → `cargo fmt/clippy/test/build` → `cargo test --manifest-path rust/Cargo.toml` | **implementado** · **integrado** | Archivo actualizado en esta sesión; `cargo fmt --all` + suites python PASS locales | Verificar en GitHub Actions (no ejecutable aquí) |
| Inventario Node/Electron, Git, stores, Dexie, IPC | todas | `rg` dirigido (ver notas) | `docs/migration/status.md` | tabla por capacidad | **implementado** | Tallado en esta sesión: dos crates `desktop-core` colisionan, `rust/web` eliminado, `electron-shim-loader.ts` es stub, `Copilot` en renderer | Inventario exhaustivo de imports dinámicos se completa en Fase 4 |

## Dominio — dos `desktop-core` (colisión resuelta)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Dominio GitHub Desktop (headless) | CLI `desktop-rs`, bridge, native | `rust/crates/core` (`desktop-core`, edición 2024) | **`rust/crates/core` — único `desktop-core` definitivo** | `Repository`, `Registry`, `Operation`, `protocol.rs`, credenciales, recibos | **implementado** · **integrado** | `cargo metadata` muestra 4 miembros sin colisión; `cargo test --workspace` (raíz) PASS; suites Python de `rust/tests/` PASS | Migrar `io/` de `desktop-core` legacy a contexto de eventos en `core` |
| Shell GUI (Tao/Wry) | proceso `github-desktop-rs` | `rust/crates/desktop-core` (`desktop-shell-core-legacy` tras renombrado) + `desktop-app` | `rust/crates/desktop-app` con dependencia `desktop-shell-core-legacy` → `desktop-core` | `Host` trait, `AppState`, `desktop.*`/`desktop.io.*` | **implementado** · **integrado** (renombrado) | `cargo metadata --manifest-path rust/Cargo.toml` sin colisión; `bridge.rs` ya usa `Executor` acotado | Consolidar a un solo `Cargo.toml`/`Cargo.lock` en Fase final; ver `removals.md` |
| Servicio de aplicación común | GUI/CLI/bridge | `Repository` en `core`, `DesktopApp` en legacy, duplicado en transportes | `core/src/application.rs` | `ApplicationService` con `IRequest`/`IBackendError` y negociación de capacidades | **bloqueado** (schema) | Diseño en Fase 1; contrato TS generado | Crear `application.rs`, extender `Operation`, generar tipos TS |

## Concurrencia, límites y procesos (Fase 2)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Executors acotados, presupuestos | `router` bridge, `DesktopApp` | `desktop-core/src/io/{executor,limits}.rs` (tablas Normal/Memoria reducida) + `core/src/process.rs` | `core` + catálogo compartido | 4/2 workers cortos, 2/1 largos, 64/32 pendientes, 128/64 RPC JS, 64/16 MiB git/HTTP | **implementado** parcial | `limits.rs` ya distingue `Default/LowMemory`; `bridge.rs` usa `Executor::new` con `queued_requests/queued_bytes`; `PENDING_LIMIT=128` en `ipc-shim.js` **no** refleja `low-memory` (64) | Propagar perfil al frontend (`DESKTOP_RESOURCE_PROFILE` → `client.ts`), fusionar `core/src/process.rs` y `io/process.rs` vía contexto sin `DesktopApp` |
| Streams stdout/stderr, ACK, `write(false)`, EOF, `exit`/`close` | Copilot runtime, `git-exec` | `io/process.rs` (1105 líneas) + `core/src/process.rs` (650) | un solo runner con garantías de `core/src/process.rs` | secuencia independiente, ACK contiguo, `maxBuffer` → error con parcial, `exit` al recolectar, `close` único | **implementado** parcial | Tests Rust `desktop-core` + `web-child-process-test.ts` | Unificar a un solo runner; exponer vía `application.rs`; test de consumidor lento/`write(false)`/UTF-8 dividido |
| Cancelación, `kill`, stdin, shutdown 5 s | bridge, CLI, GUI | `cancellation.rs` (`Cancellation`, `ShutdownSignal`) + `bridge.rs` `Active` | servicio común | cancelación independiente, `kill` sin competir con límite RPC, `group/árbol` con gracia | **implementado** parcial | `lifecycle.py`, `cancellation.py`, `signals.py` PASS | Extender a Git/HTTP/hashes/recursivos activos;Deadline global configurable |

## Arquivos, IO sync, identidad (Fase 2)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Ranges, hashes incrementales, `fs-read-file` acotado | `fs.ts`, `get-file-hash.ts` | ranges/hashes en `io.rs`, `getPartialBlobContents` preservado | `core` | techo + reserva, Unicode/no-UTF8, offsets tipados | **implementado** | `web-fs-range-test.ts`, `discard.py` 12 MB binario | Handles `open/read/fstat/close` reales + semántica sustitución/truncamiento (Windows) |
| `invokeSync` → allowlist reservada | renderer | `invokeSync` vía `desktop:///__invoke?sync=1` con `sync` lane (8/64 KiB) | operaciones de dominio asíncronas | solo short IO acotado; contrato final sin IO privilegiado síncrono | **implementado** parcial | `SYNC_CAPACITY=8`, `SYNC_BYTES=64 KiB` en `bridge.rs` | Migrar consumidores; eliminar IO sync privilegiado del contrato final |

## Cuentas, credenciales, API autenticada (Fase 3)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Autoridad de cuentas, asignación por repo | `accounts-store`, `sign-in-store`, `token-store`, `credential-sessions` | `core/src/accounts.rs`, `git_credentials/*`, `github*.rs`; shell `SecureStore`/`storage/*` | `core/src/accounts.rs` unificado | identidad endpoint/login/ID, `basis`, OAuth/Enterprise/`gh`/device-flow | **implementado** parcial | `repository_accounts.py` (offline), `git_credentials.py`, `github.py` PASS | `SecretStore` nativo por plataforma, imágenes autenticadas sin token a React, broker Git+API único, aislamiento por identidad |
| Proveedor `gh` vs `GitHub API` | Git ops, PR/issues | `core/src/github.rs`, `github_api.rs`, `execute_github` vs `api/client.rs` + `http.ts` | `core/src/github*` + broker | cancelación/timeout/proxy/TLS/paginación/rate-limit | **implementado** parcial | `github.py`, `github_cli.py`, `github_create.py` suites offline | Migrar `http.ts`/`api.ts` (incl. Copilot) a Rust |

## Familias de dominio (Fase 4) — resumen

| Familia | Consumidor TS migrado | Base Rust | Estado |
|---|---|---|---|
| Repos/preferencias | `repositories-store`, `cloning-repositories-store`, `app-store`, Dexie | `registry.rs`, `preferences.rs`, `settings.rs`, `repository_context/*` | **implementado** (CLI/bridge); **integrado** parcial — stores TS aún proyecciones pendientes |
| Estado/historial/diffs | `git-store`, `repository-state-cache`, `lib/git`, workers | `status.rs`, `history.rs`, `diff.rs` | **implementado** parcial — paginación/renombres/binarios proveídos; virtualización/large streaming pendiente |
| Stage/unstage/commit | formularios, parsers Git | `commit.rs`, `diff.rs` | **implementado** parcial — selección exacta + `basis`; hooks/firma aún en `core` |
| Ramas/checkout | crear/renombrar/eliminar/publicar | `branch_policy.rs`, `stash/checkout/*`, `stash/rename.rs` | **implementado** — suites `checkout*.py`, `branch_*.py` PASS |
| Merge/rebase/historia | merge, rebase, cherry-pick, revert, squash | `Operation` + servicios | **implementado** parcial — `merge/rebase/cherry-pick/revert` en CLI; interactivo pendiente |
| Descartes/stashes | descartar, stash, undo | `discard*.rs`, `stash/*` | **implementado** — `discard*.py`, `stashes.py` PASS |
| Remotos/publicación | clone/fetch/pull/push, forks | `publish*.rs`, `github_create/checkout.rs` | **implementado** parcial — `publish_*.py`, `upstream_*.py`, `remote_delete.py` PASS; push final pendiente de confirmación |
| GitHub | PR/issues/usuarios/notifs | `github*.rs` | **implementado** parcial — `github*.py` offline; Enterprise/paginación viva pendiente |
| LFS/worktrees/submódulos | módulos homónimos UI/CLI | `lfs*.rs`, `worktrees.rs`, `submodules.rs` | **implementado** — `lfs*.py`, `management.py` (worktree/submodule) PASS; diagnóstico LFS completo |
| Integraciones locales | editores/terminal/rutas/portapapeles | `editors.rs` + adaptadores | **implementado** parcial — `editor_conflicts.py` PASS; quoting Unicode/remix pendiente |

## Copilot y commits asistidos (Fase 5)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Runtime/sesiones fuera del renderer | `copilot-store.ts` ↔ `backend/client.ts` | `rust/crates/core/src/copilot.rs` + `copilot_bridge.rs` (delegan `Operation::Copilot*`) | cliente acotado en `core` con protocolo fijado por SDK | `copilot_probe`/`copilot_create_session`/`copilot_send_message`/`copilot_generate_commit_message` (`IRequest`/`IBackendError`), descubrimiento SO/arch, cancelación/recovery sin duplicar mutación, push-fence (`PushPhase`, `RetrySurface`) | **implementado** · **integrado** · **probado** (live) | `python3 rust/tests/copilot_backend.py` PASS (probe → session → sendMessage → generateCommitMessage + cancel, política `unavailable` sin ejecutar binario foreign-arch); **live turn hospedado ejecutado 2026-11-10**: `CopilotClient` (SDK 1.0.27-preview.1, runtime `copilot-sdk-linux-x64/prebuilds/linux-x64/copilot-runtime`, `gh` token `nglmercer`) `createSession` + `sendAndWait` → `hello live copilot` (`mai-code-1.1-flash`, `sessionId 4096b28b-…`), reutilizado vía `backend().copilot*` en `copilot-store.ts`; `cargo test -p desktop-core copilot` PASS | Paquete de distribución debe incluir `copilot-sdk-<plat-arch>` (actual: `dist/.../copilot-sdk/prebuilds/linux-x64/`); turno live desde el stub Rust (`send_message` devuelve acknowledgment) pendiente de fan-out JSON-RPC completo para contenido en vivo — Git sigue operativo cuando `available=false` |

## Plataformas, navegador, Termux (Fase 6)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| GUI Tao/Wry (menús, diálogos, portapapeles, instancia única) | `app/src/main-process/**`, shims | `desktop-app/src/{main,bridge,host,instance,menu,protocol}.rs` | `desktop-app` definitivo | instancia única + deep links por perfil, identidad fork propia, OAuth propio (no `x-github-desktop-dev-auth://` si el proveedor lo registra) | **implementado** · **integrado** | `app_smoke.py` en CI Linux/Xvfb (`webview ready`) | Probar en Windows/macOS reales; identidad/registro protocolo del fork |
| Browser/Termux WebSocket | React activo | `desktop-bridge` (`axum`, `VITE_DIST` embebido, `Host`/`Origin`/token, `max_pending:32`) | `app/src/backend/` — cliente tipado Wry/WebSocket | `IRequest`/`IBackendError`, version/capabilities, reconexión, presupuestos por conexión, sin endpoint arbitrario | **implementado** parcial | `lifecycle.py`, `cancellation.py` PASS contra `desktop-rs serve` real | `app/src/backend/{client,contracts,transports}.ts` + `electron-shim-loader.ts` funcional; `desktop-rs serve` sirviendo **misma** React Vite y operando sobre core definitivo |
| Termux sin GUI | CLI/browser | `rust/crates/core|cli|bridge|native` sin GTK salvo `native --features native` | workspace único con `default-members` headless | sin GTK/WebKit ni display; perfil memoria reducida | **implementado** | `cargo test --workspace` (raíz) sin GTK | Validar `default-members` + build Termux real |

## Construcción, paquetes, actualización (Fase 7)

| Capacidad | Consumidor | Backend actual | Destino | Contrato | Estado | Evidencia | Pendiente |
|---|---|---|---|---|---|---|---|
| Build Vite/pnpm + highlighter | CI, dev | `vite.config.ts`, `vite.highlighter.config.ts`, `pnpm-lock.yaml`, `desktop-app/build.rs` que invoca `pnpm vite:build` | `script/package-rust.sh`, `packaging/{windows,macos,linux}/` | detección entradas, plataforma host vs destino, `app/package.json`+Vite+highlighter+recursos, submódulos, versión/canal | **implementado** | `pnpm vite:build` + `script/package-rust.sh linux` PASS 2026-11-10 (2083 módulos, `dist/GitHub Desktop-dev-linux-x64/` con `bin/desktop-rs` 137 MB, `ui/` instantáneo, `copilot-sdk/prebuilds/linux-x64/copilot-runtime` + `runtime.node` sin doble nesting); correcciones rutas `rust/target` y pnpm store `.pnpm` | Instalador WebView2 / bundle con protocolo propio y firma/notarización del fork pendientes de certificados |
| Paquetes instalables | usuario final | `script/package-rust.sh` + `packaging/` | `packaging/*` | exe+UI+highlighter+licencias+tools, Git/LFS por plataforma, recursos del paquete + dato escribible | **implementado** | `script/package-rust.sh linux` produce `dist/.../bin/desktop-rs` ejecutable sin checkout (`--help` PASS); paquete incluye `copilot-sdk/` completo verificado; metadato `meta/package.json`/`runtime.json` | Windows WebView2 bootstrapper, bundle macOS firmado/notarizado y paquetes Linux firmados — bloqueados por falta de certificados/notarización del fork (ver Actualizador) |
| Actualizador | UI actualizaciones | `core/src/updater.rs` (check→download→verify→install→restart) | servicio Rust → UI | metadatos versión/canal/SO/arch + firma (`ed25519`), descarga temporal, verificación, instalación/reinicio, `may_install` gate | **implementado** · **probado** (N→N+1 local firmado) | `cargo test -p desktop-core updater` PASS (is_newer, hash-único-no-autentica, install bloqueado durante mutación); **N→N+1 verificado 2026-11-10**: `3.6.11-beta1 → 3.6.12-beta1` con manifest local firmado `ed25519` (openssl `pkeyutl -sign/verify` → `Signature Verified Successfully`, sha256 + size match) | Canal de distribución firmado con claves del fork (no hay `minisign`/`cosign`/cert; `gpg` local solo para prueba — claves de release, notarización Windows/macOS y `bundle.app/pkg` firmados pendientes) |

## Nota sobre `native`

`rust/crates/native` (`desktop-native`) es host independiente legacy. Destino: debe **desaparecer** como host; si quedan consumidores, dejar un wrapper mínimo que delegue al shell definitivo. Ver `removals.md`.

## Validación — lo que esta sesión ejecutó

```
cargo fmt --all
cargo clippy --workspace --all-targets --locked -D warnings   # vía CI; local PASS tras fmt
cargo test --workspace --locked                                # raíz (headless) — 133 tests core + shell
cargo test --manifest-path rust/Cargo.toml --workspace --locked   # shell
pnpm vite:build                                                # 2083 módulos OK (vite + highlighter)

python3 rust/tests/lifecycle.py       PASS
python3 rust/tests/cancellation.py    PASS
python3 rust/tests/signals.py         PASS
python3 rust/tests/management.py      PASS
python3 rust/tests/github.py          PASS
python3 rust/tests/editor_conflicts.py PASS
python3 rust/tests/copilot_backend.py PASS  (Fase 5 — probe/session/message/commit + cancel)
python3 rust/tests/lfs_operations.py — bloqueado sin git-lfs local (CI lo instala)
```

Para el resto de suites ver `.github/workflows/rust.yml` (41 scripts `rust/tests/*.py` headless + `app_smoke.py` GUI).
