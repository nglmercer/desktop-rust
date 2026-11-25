# Retiradas — desktop-rust

Este documento distingue lo que ya salió del flujo activo de lo que todavía es
un candidato de retirada. No uses entradas históricas como evidencia actual: la
prueba vigente es la que se puede ejecutar desde este checkout.

## 1. Ya retirado del flujo activo

| Ruta o pieza | Sustitución actual | Evidencia rutinaria |
|---|---|---|
| `app/src/main-process/**`, entradas Electron y crash shell | `rust/crates/desktop-app` (Tao/Wry) | `pnpm build:dev`, `cargo test -p desktop-app` y smoke Xvfb en CI |
| `app/webpack.*`, `script/build.ts`, `script/package.ts`, `script/post-install.ts` | Vite + `script/package-rust.ts` | `pnpm vite:build`, `pnpm package` |
| Electron, electron-packager/installer/devtools y dependencias exclusivas | host Rust nativo | root `package.json`/`pnpm-lock.yaml` sin esas dependencias |
| `package-lock.json` raíz y `app/package-lock.json` | `pnpm-lock.yaml` | `pnpm install --frozen-lockfile --ignore-scripts` |
| ESLint, typescript-eslint, plugins ESLint y Prettier raíz | oxlint + reglas `oxlint-rules/` + Biome format | `pnpm lint`, `pnpm test:oxlint` |
| `eslint-rules/`, `biome-rules/`, `biome.security.json`, `.eslintrc*`, `.prettierrc*` | `.oxlintrc.json`, `.oxlint.security.json`, `biome.json` | `pnpm lint`, `pnpm format:check` |
| `vitest-suites.json` y enrutado explícito de suites | Vitest como único runner | `pnpm test`, `pnpm test:script` |
| Suites Python ya reemplazadas | tests Rust/Vitest equivalentes | `cargo test`, `pnpm test` |
| `rust/Cargo.toml`/`rust/Cargo.lock` separados | workspace Cargo raíz | `cargo metadata`, `cargo test --workspace` |
| `ipc-shim.js` fuente manual | `web/ipc-shim.ts` compilado por Vite | `pnpm vite:build` |
| `script/run.ts` (launcher del paquete Electron) | `script/start-rust.ts` / `script/start-rust-package.ts` | `pnpm compile:script` |
| `script/copilot.ts` y su test aislado de copia npm del runtime | empaquetado Rust en `script/package-rust.ts` | `pnpm compile:script`, `pnpm test:script` |
| `app/src/lib/compute-bundle-hash.ts` y metadatos de instaladores/NuGet/bundle Electron en `script/dist-info.ts` | empaquetado Rust/Vite vigente | `pnpm compile:script`, `pnpm check:types` |
| `find-toast-activator-clsid.ts`, GUIDs Electron de actualizador/main, `set-state.ts`, `logging/get-log-path.ts` y `actions-log-parser/` | sin consumidores; notificaciones/actualización/host Rust vigentes | `pnpm check:types`, `pnpm lint` |
| fallback de tamaño de archivos por `fs.stat` | `renderer_large_files` en backend Rust | `app/test/unit/rust-large-files-routing-test.ts` |
| lectura directa de `static/licenses.json` mediante `fs` | recurso servido por el host mediante `readBundledTemplate` | `app/test/unit/acknowledgements-test.tsx` |
| lectura directa de `emoji.json` mediante `fs` | recurso servido por el host y `fetch` del bundle | `app/test/unit/read-emoji-test.ts` |
| configuración Git arbitraria por shell en renderer | contratos Rust tipados y allowlist de settings consumidos por la app | `app/test/unit/rust-global-git-identity-test.ts`, `app/test/unit/git/config-test.ts` |
| fallback backend-less de creación de commits | checkpoint Rust `prepareSelectedCommit`/`publishSelectedCommit` obligatorio | `app/test/unit/rust-native-commit-routing-test.ts`, `app/test/unit/git/commit-test.ts` |
| descubrimiento de editores por `child_process`/archivos específicos de Darwin, Linux y Windows | descubrimiento validado del host Rust | `pnpm check:types`, integración backend `available_editors` |
| fallback backend-less de creación de repositorios y commit inicial | inicialización, archivos y commit inicial Rust obligatorios | `app/test/unit/ui/assisted-repository-creation-test.tsx` |
| lectura de `pretext-draft.md` mediante `fs` y constantes de imágenes sin consumidores | recurso servido por el bundle/host | `app/test/unit/release-notes-test.ts` |
| diálogo legacy de borrado de `config.lock` y parsing de errores de config lock | escritura de configuración Rust tipada; errores genéricos sin borrado renderer | `app/test/unit/configure-git-user-save-test.tsx`, `app/test/unit/git/core-test.ts` |
| validación backend-less de rutas/argumentos de integraciones personalizadas | validación de host Rust obligatoria | `app/test/unit/native-custom-integration-test.ts` |
| serialización IPC de menús contextuales y fallback `show-contextual-menu` | menús contextuales DOM-owned usados siempre por el renderer | `app/test/unit/ui/commit-message-test.tsx`, `app/test/unit/ui/assisted-commit-history-test.tsx` |
| lectura directa del destino de clone mediante `readdir` | estado de destino Rust disponible por WebSocket y Tao/Wry | `app/test/unit/clone-destination-websocket-test.tsx`, `app/test/unit/ui/clone-repository-test.tsx` |
| lectura directa de `markdown.css` mediante `fs` | hoja servida como recurso del bundle y obtenida con `fetch` same-origin | `app/test/unit/sandboxed-markdown-test.tsx` |
| lectura directa de emoji mediante URLs `file:` | solo emoji servidos por el origen/bundle Rust | `app/test/unit/sandboxed-markdown-test.tsx` |
| lectura directa de archivos de trabajo para syntax highlighting | snapshot acotado `renderer_working_file` Rust obligatorio | `app/test/unit/rust-syntax-highlighting-test.ts` |
| lectura directa de `FETCH_HEAD` para fecha de fetch | lectura Rust `repositoryLastFetch` obligatoria | `app/test/unit/stores/last-fetch-test.ts` |
| `readPartialFile` sin consumidores de producción | lecturas acotadas Rust del archivo de trabajo | `app/test/unit/file-system-test.ts` |
| ejecución directa de `git reset` hard/soft en renderer | reset revisado Rust por WebSocket y Tao/Wry | `app/test/unit/git/reset-test.ts`, `cargo test -p desktop-core renderer_reset`, `cargo test -p desktop-app wry_resets_a_reviewed_commit` |
| lanzamiento backend-less de editores externos y edición de configuración global | apertura de editores validada por el host Rust | `pnpm check:types`, integración backend `open_editor` |
| fallback backend-less de descarte de cambios y borrado directo de archivos en `GitStore` | descarte revisado/recuperable del backend Rust obligatorio | `app/test/unit/rust-reviewed-discard-test.ts`, `app/test/unit/rust-batch-discard-dialog-test.tsx`, `app/test/unit/rust-discard-recovery-test.tsx` |
| diálogo legacy de descarte con recuperación por papelera y lista no revisada | revisión Rust obligatoria con recuperación exacta | `app/test/unit/rust-batch-discard-dialog-test.tsx` |
| comprobación duplicada de archivos grandes en sidebar y exclusión condicional de recuperación de descartes | revisión grande integrada en `prepareSelectedCommit`; recuperación siempre disponible | `app/test/unit/ui/commit-message-test.tsx`, `app/test/unit/rust-reviewed-discard-test.ts` |
| puerta `transportKind` del diálogo de borrado remoto de ramas | revisión y recuperación Rust obligatorias para borrado remoto | `app/test/unit/ui/rust-remote-branch-deletion-dialog-test.tsx`, `app/test/unit/rust-remote-branch-deletion-test.ts` |
| lectura de archivos de trabajo sin lease en WebSocket para highlighting | lectura uniforme bajo lease de operación de repositorio | `app/test/unit/rust-syntax-highlighting-test.ts` |
| fallback shell para eliminar HEAD inicial en revert del primer commit | eliminación revisada Rust obligatoria | `app/test/unit/rust-first-undo-routing-test.ts` |
| comprobación duplicada de capacidad de progreso de commit en `createCommit` | validación única en el cliente backend tipado | `app/test/unit/git/commit-test.ts`, `app/test/unit/rust-native-commit-routing-test.ts` |
| wrapper TypeScript sin consumidores para `diff-index`/estado del índice | operaciones de índice Rust usadas por flujos vivos; constante null-tree local a `diff.ts` | `app/test/unit/git/diff-test.ts` |
| wrappers TypeScript sin consumidores para `checkoutIndex`, `resetPaths` y `resetSubmodulePaths` | operaciones Rust internas de descarte/checkout/staging ya revisadas; batching compartido conservado | `app/test/unit/git/reset-test.ts`, `app/test/unit/git/submodule-test.ts`, `app/test/unit/rust-submodule-routing-test.ts` |
| `getGlobalConfigPath` sin consumidores de producción | edición de configuración global abierta directamente por el host Rust | `app/test/unit/rust-global-git-identity-test.ts`, `app/test/unit/git/config-test.ts` |
| `DiscardChangesError` y handler legacy de fallos al mover a papelera | descarte revisado/recuperable Rust obligatorio | `pnpm check:types`, `pnpm lint` |
| `getRemoteURL` sin consumidores de producción y su fallback shell para URLs sensibles | lectura de remotes segura Rust; tests de fixtures usan `getRemotes` | `app/test/unit/git/remote-management-test.ts`, `app/test/unit/rust-remote-presence-test.ts` |
| fallback directo de release notes mediante `fetch` cross-origin en Tao/Wry | operación Rust `release_changelog` compartida por WebSocket y Tao/Wry | `app/test/unit/release-changelog-routing-test.ts`, `app/test/unit/release-changelog-client-test.ts` |
| canal host `show-contextual-menu` y menú DOM duplicado del shim Wry | menús contextuales DOM-owned del renderer mediante `showBrowserContextMenu` | `app/test/unit/ipc-contract-test.ts`, `app/test/unit/wry-fetch-progress-test.ts` |
| `app/src/lib/process/win32.ts` y helper macOS Catalina sin consumidores | operaciones de terminal/proceso vigentes en host Rust o módulos plataforma vivos | `pnpm check:types`, `app/test/unit/rust-terminal-dispatch-test.ts` |
| helpers renderer sin consumidores de terminal output, revert progress, buffers, `pick` y SHA tooltip | flujos Git/progreso vigentes en backend Rust o UI actual | `pnpm check:types`, `app/test/unit/rust-abort-operation-routing-test.ts` |
| fallback shell general de `deleteRef` sin consumidores de producción | única operación viva acotada a eliminación revisada de HEAD inicial | `app/test/unit/rust-submodule-routing-test.ts`, `app/test/unit/rust-first-undo-routing-test.ts` |
| inicialización, README, commit inicial y remote de tutorial mediante filesystem/shell directos | operaciones Rust de creación de repositorio ya usadas por el diálogo de nuevo repositorio | `app/test/unit/tutorial-repository-accounts-test.ts` |
| lectura filesystem backend-less de conflictos para contexto Copilot | captura certificada/acotada Rust obligatoria bajo lease uniforme | `app/test/unit/copilot-conflict-context-native-test.ts`, `app/test/unit/copilot-conflict-context-test.ts` |
| staging de resoluciones Copilot mediante `git add` directo | `update-index` Rust literal por lotes bajo lease | `app/test/unit/copilot-conflict-resolution-test.ts`, `app/test/unit/stores/copilot-store-test.ts` |
| fallbacks backend-less de revisión de commit, revisión de checkout, abort squash y certificados Copilot en `AppStore` | operaciones revisadas Rust obligatorias | `app/test/unit/git/commit-test.ts`, `app/test/unit/ui/stash-and-switch-branch-dialog-test.tsx`, `app/test/unit/git/checkout-test.ts` |
| selección nativa condicional de generación de mensajes Copilot | selección tipada Rust siempre disponible cuando el SDK está habilitado | `app/test/unit/ui/commit-message-test.tsx`, `app/test/unit/copilot-commit-message-prompt-test.ts` |
| puertas `transportKind` de UI de clone nativo y recuperaciones | UI nativa disponible siempre; la selección explícita del usuario sigue controlando la operación | `app/test/unit/ui/clone-repository-test.tsx`, `app/test/unit/clone-destination-websocket-test.tsx` |
| detección Linux de Git/terminales con fallback de proceso renderer | consulta obligatoria al host Rust en Linux | `app/test/unit/rust-terminal-dispatch-test.ts` |
| descubrimiento/lanzamiento Linux de terminales duplicado en renderer | lista fija y lanzamiento confinado del host Rust (`available_terminals`/`open_terminal`) | `app/test/unit/rust-terminal-discovery-test.ts`, `app/test/unit/rust-terminal-dispatch-test.ts` |
| loader síncrono `source-map-support` acoplado a nombres webpack/Electron y filesystem renderer | normalización explícita de errores con stack original; mapas Vite quedan fuera del bundle y la dependencia fue retirada | `app/test/unit/normalize-error-test.ts`, `pnpm vite:build` |
| lecturas `rev-parse` del contexto de locks padre HEAD/ref (`format`, rutas Git y valor esperado) | operación tipada Rust `ref_lock_context`; el valor se vuelve a validar después de adquirir los locks | `cargo test -p desktop-core ref_lock`, `cargo test -p desktop-app wry_ref_lock_context`, `app/test/unit/backend-websocket-test.ts` |
| fallback genérico de apertura de carpetas para repositorios guardados | operación confinada `reveal_repository`/`reveal_registered_repository` en WebSocket y Tao/Wry, sin IPC genérico ni apertura de paquetes ejecutables | `app/test/unit/reveal-repository-client-test.ts`, `app/test/unit/reveal-repository-routing-test.ts`, `cargo test -p desktop-app wry_reveal_repository` |
| parche renderer de entorno de shell en macOS (`shell.ts`) | eliminado: mutar el shim `process.env` del webview no afecta al host Rust ni a procesos Git reales | `pnpm check:types`, `pnpm vite:build` |
| lecturas renderer de HEAD, rama, tip y Git dir para assisted-commit | operación tipada Rust `assisted_commit_head`, con estados inseguros devueltos como datos validados | `cargo test -p desktop-core assisted_head`, `cargo test -p desktop-app wry_assisted_commit_head`, `app/test/unit/backend-websocket-test.ts`, `app/test/unit/git/assisted-commit-test.ts` |
| comprobación renderer `which`/`where.exe` de Git al abrir terminal | `git_version` revisado del host Rust en todas las plataformas | `app/test/unit/rust-terminal-dispatch-test.ts` |
| canonicalización filesystem directa en `resolveWithin` | canonicalización host Rust tipada | `app/test/unit/path-test.ts` |
| `isGitOnPath` sin consumidores tras la detección obligatoria de Git por host | `findGitOnPath` conservado solo para el alias Git de CMD en Windows | `pnpm check:types`, `pnpm lint` |
| imports directos de tipos `ChildProcess` en rebase/update-ref/progreso/terminales | interfaz estructural mínima `IChildProcess`, preparada para procesos host o shim web | `pnpm check:types`, `app/test/unit/rust-rebase-state-routing-test.ts`, `app/test/unit/pull-hook-callbacks-test.ts` |
| apertura genérica de carpetas sin consumidores (`showFolderContents`, `is-application-bundle` y canales `unsafe-open-directory`/`confirm-reveal-directory`) | apertura confinada de repositorios registrados; el host usa reveal seguro sin clasificar metadata en el renderer | `app/test/unit/reveal-repository-routing-test.ts`, `app/test/unit/ipc-contract-test.ts`, `cargo check --workspace --locked` |
| fallback legacy de `fetch`/`fetchRefspec` y resolución proxy incompatible con IPC | fetch revisado Rust obligatorio; proxies explícitos pasan por contrato tipado y se redactan en diagnósticos | `app/test/unit/rust-fetch-routing-test.ts`, `app/test/unit/git/fetch-test.ts`, tests Rust `renderer_fetch`/`process` |
| fallbacks shell de descubrimiento/alta/borrado/URL/HEAD de remotes | descubrimiento seguro y mutaciones revisadas Rust obligatorias; URLs ambiguas o con credenciales se rechazan explícitamente | `app/test/unit/rust-remote-presence-test.ts`, `app/test/unit/git/remote-discovery-test.ts`, `app/test/unit/git/remote-management-test.ts` |
| imports renderer directos de `crypto` y `keytar` | boundary web explícita ya respaldada por Rust (`web/crypto`, `web/keytar`) | `pnpm check:types`, `pnpm lint`, `pnpm vite:build` |

## 2. Retirado como dependencia raíz, conservado temporalmente como fuente o shim

Estas piezas ya no son dependencias nativas del runtime Electron, pero sus
fuentes o shims siguen presentes porque todavía tienen consumidores migrando:

| Pieza | Estado actual | Condición de borrado final |
|---|---|---|
| `keytar`, `registry-js`, `fs-admin` | imports dirigidos a shims web o Rust | ningún consumidor ni test dependa del shim |
| `desktop-notifications`, `windows-argv-parser` | fuentes vendorizadas/resolución de tipos para consumidores existentes | reemplazo Rust/tipado completo en notificaciones e integración de comandos |
| `desktop-trampoline` | dependencia real en `app/package.json` | entorno Git/SSH/askpass servido completamente por Rust |
| `printenvz` | helper Rust vendorizado usado por scripts de entorno de hooks | integración de hooks migrada o eliminada |
| `dugite` | dependencia real mientras existen fallbacks Git Node | 0 imports del renderer y tests usando Rust |
| shims `app/src/web/*` | compatibilidad para renderer/tests | retirar archivo por archivo tras eliminar consumidores |

## 3. Candidatos estructurales pendientes

| Candidato | Condición antes de eliminar |
|---|---|
| `rust/crates/desktop-shell-core-legacy` | servicios útiles integrados en `rust/crates/core`; GUI/CLI/bridge sin dependencia legacy |
| `rust/crates/native` como host independiente | shell definitivo validado en plataformas; conservar wrapper fino solo si hay consumidores del ejecutable |
| fallbacks `transportKind` en renderer | cada módulo probado contra backend Rust y sin consumidores backend-less |
| llamadas legacy `git()` en `app/src/lib/git` | operaciones Rust tipadas con paridad, progreso, cancelación y recuperación |
| persistencia/proyecciones TS que duplican dominio | autoridad de dominio en Rust y contratos TypeScript generados |
| scripts npm residuales de release | publicación/versionado verificados con pnpm/Rust sin npm CLI |
| documentación histórica contradicha por el flujo actual | reemplazada por README, docs de contribución, `rust/MIGRATION.md` y `rust/ROADMAP.md` actuales |

La línea base numérica viva está en `pnpm migration:status` (11 comprobaciones
`transportKind`, 28 llamadas legacy Git, 17 imports `dugite`, 17 imports `fs`,
0 suites en el runner Node). `rust/ROADMAP.md` ordena el trabajo restante.

## 4. Qué se conserva

React/Primer, componentes, estilos, recursos vigentes, highlighter, submódulos
`gemoji`, `gitignore`, `choosealicense.com`, licencias, procedencia y oráculos
útiles. No se borran por parecer antiguos ni por ausencia en un import TS.
