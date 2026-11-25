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
| lectura directa `lstat` de entradas assisted-commit para freshness preflight | operación tipada Rust `renderer_working_file_metadata` con rutas confinadas y versión opaca validada | `app/test/unit/assisted-commit-selection-test.ts`, `app/test/unit/backend-client-session-test.ts`, `cargo test -p desktop-core renderer_working_file::tests::metadata_versions_are_bounded_literal_and_missing_aware` |
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
| componentes UI huérfanos `HorizontalRule`, `ToggleButton`, `Caption` y estilo horizontal exclusivo | componentes de diálogo/input description vigentes usados por producción | `app/test/unit/ui/component-primitives-test.tsx`, `app/test/unit/ui/dialog-composition-and-toggle-test.tsx`, `app/test/unit/ui/dialog-and-input-description-test.tsx` |
| wrapper y operación standalone `renderer_large_files` sin consumidores de producción | revisión de archivos grandes integrada en `prepareSelectedCommit`/commit preview Rust | `app/test/unit/rust-native-commit-routing-test.ts`, `cargo check -p desktop-core -p desktop-app --locked` |
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
| lecturas renderer de HEAD, rama, tip, Git dir, raíz de worktree, formato de refs, resolución confinada de rutas Git, captura de entradas del índice, captura estructurada de configuración/refs y comprobación de operaciones/conflictos para assisted-commit | operaciones tipadas Rust `assisted_commit_head`/`assisted_commit_safety`/`assisted_commit_git_path`/`assisted_commit_index_entries`/`assisted_commit_refs`/`assisted_commit_config`, con estados inseguros o bytes del índice devueltos como datos validados | `cargo test -p desktop-core assisted_head`, `cargo test -p desktop-app wry_assisted_commit_head`, `app/test/unit/backend-websocket-test.ts`, `app/test/unit/git/assisted-commit-test.ts` |
| comprobación renderer `which`/`where.exe` de Git al abrir terminal | `git_version` revisado del host Rust en todas las plataformas | `app/test/unit/rust-terminal-dispatch-test.ts` |
| canonicalización filesystem directa en `resolveWithin` | canonicalización host Rust tipada | `app/test/unit/path-test.ts` |
| `isGitOnPath` sin consumidores tras la detección obligatoria de Git por host | `findGitOnPath` conservado solo para el alias Git de CMD en Windows | `pnpm check:types`, `pnpm lint` |
| imports directos de tipos `ChildProcess` en rebase/update-ref/progreso/terminales | interfaz estructural mínima `IChildProcess`, preparada para procesos host o shim web | `pnpm check:types`, `app/test/unit/rust-rebase-state-routing-test.ts`, `app/test/unit/pull-hook-callbacks-test.ts` |
| apertura genérica de carpetas sin consumidores (`showFolderContents`, `is-application-bundle` y canales `unsafe-open-directory`/`confirm-reveal-directory`) | apertura confinada de repositorios registrados; el host usa reveal seguro sin clasificar metadata en el renderer | `app/test/unit/reveal-repository-routing-test.ts`, `app/test/unit/ipc-contract-test.ts`, `cargo check --workspace --locked` |
| imports renderer escritos como builtin `os` aunque Vite ya los resolvía al shim host | frontera explícita `app/src/web/os.ts`; queda prohibido reintroducir `from 'os'` fuera de `web/` | `app/test/unit/file-system-test.ts`, `app/test/unit/copilot-app-test.ts`, `app/test/unit/macos-version-from-darwin-test.ts`, `pnpm migration:check` |
| ramas `transportKind` de clone/recovery nativo en la UI | métodos tipados `cloneSelectedRepository`, `listNativeCloneRecoveries` y `recoverNativeClone` del cliente backend | `app/test/unit/native-clone-ui-test.ts`, `app/test/unit/backend-client-session-test.ts` |
| puerta `transportKind` de selección de repositorio browser en `AppStore` | flujo tipado `selectBrowserRepositoryAtPath` que clasifica la ruta y limpia la selección cuando no es un working tree regular | `app/test/unit/backend-client-session-test.ts`, `app/test/unit/browser-repository-selection-test.ts` |
| fallback legacy de `fetch`/`fetchRefspec` y resolución proxy incompatible con IPC | fetch revisado Rust obligatorio; proxies explícitos pasan por contrato tipado y se redactan en diagnósticos | `app/test/unit/rust-fetch-routing-test.ts`, `app/test/unit/git/fetch-test.ts`, tests Rust `renderer_fetch`/`process` |
| fallback proxy/inherited-proxy de push nativo | proxies explícitos pasan por el mismo contrato tipado que fetch; proxies heredados permanecen confinados al entorno del proceso host y ambos se redactan en diagnósticos | `app/test/unit/rust-push-routing-test.ts`, `app/test/unit/git/environment-test.ts`, test Rust `renderer_proxy_validation_and_debug_protect_credentials` |
| ramas `transportKind` de capacidad/progreso/proxy en push nativo | intento nativo uniforme; solo errores tipados explícitamente pre-mutación (`unsupported`, `no_transport`, URL o hooks no soportados) pueden usar el fallback legacy | `app/test/unit/rust-push-routing-test.ts` |
| fallback proxy/inherited-proxy de pull nativo | proxies explícitos pasan por contrato tipado al igual que fetch/push; el helper fetch sin proxy quedó retirado porque todas las operaciones usan la forma explícita | `app/test/unit/rust-pull-routing-test.ts`, `app/test/unit/rust-pull-progress-ownership-test.ts` |
| fallbacks shell de descubrimiento/alta/borrado/URL/HEAD de remotes | descubrimiento seguro y mutaciones revisadas Rust obligatorias; URLs ambiguas o con credenciales se rechazan explícitamente | `app/test/unit/rust-remote-presence-test.ts`, `app/test/unit/git/remote-discovery-test.ts`, `app/test/unit/git/remote-management-test.ts` |
| imports renderer directos de `crypto` y `keytar` | boundary web explícita ya respaldada por Rust (`web/crypto`, `web/keytar`) | `pnpm check:types`, `pnpm lint`, `pnpm vite:build` |
| escritura directa de resoluciones Copilot mediante `writeFile` del renderer | contenido subido en chunks acotados de sesión y escrito bajo lease por operación confinada Rust `renderer_write_working_file`; staging posterior sigue bajo lease | `cargo test -p desktop-core write_working_file`, `cargo test -p desktop-app wry_resolution_preview_upload_and_read`, `app/test/unit/backend-websocket-test.ts`, `app/test/unit/stores/app-store-add-accounts-test.ts` |
| wrapper compartido `exec-file` ejecutado directamente con `child_process` | ejecución a través del shim host `web/child_process`, con `timeout`, `maxBuffer`, señal de terminación y kills pendientes antes de asignar id | `app/test/unit/web-child-process-test.ts`, `app/test/unit/copilot-app-test.ts` |
| lanzamiento de integraciones personalizadas importando directamente `child_process` | spawn host-backed `web/child_process` con stdio ignorado; el host preserva procesos detached en sesión propia y fuera del apagado del renderer, y la interfaz de shells consume la abstracción estructural `IChildProcess` | `app/test/unit/native-custom-integration-test.ts`, `cargo test -p desktop-shell-core-legacy io::process::tests` |
| creación y limpieza de archivos de progreso LFS mediante imports directos `fs` | operaciones host-backed `web/fs`/`web/fs-promises`; la creación conserva exclusividad `wx` | `app/test/unit/web-fs-lfs-progress-test.ts` |
| imports dispersos del enum `GitError` de dugite en consumidores renderer | vocabulario centralizado en `app/src/lib/git/error-kind.ts`; quedan imports directos solo en la frontera del vocabulario y el legacy runner | `pnpm check:types`, `app/test/unit/rust-fetch-error-test.ts`, routing fetch/pull/push |
| resolución dugite del binario Git para hooks y tabla vacía de señales del shim `os` | ejecución mediante `git` resuelto por el host/PATH, como ya ocurría en el bundle web; códigos de salida por señal POSIX conservados | `app/test/unit/traditional-hook-test.ts` |
| imports directos `dugite` en hooks/credenciales | frontera explícita host-backed `app/src/web/dugite.ts`; los tests inyectan un bridge Node solo para ejercitar la superficie web | `app/test/unit/hooks-discovery-test.ts`, `app/test/unit/git/credential-test.ts` |
| `fs.createWriteStream` ausente en el shim host usado para stdin de hooks | stream Writable real con creación/truncado inicial y appends chunked por el host; conserva destinos vacíos | `app/test/unit/web-fs-write-stream-test.ts` |
| import directo `fs/promises` en descubrimiento de hooks | frontera explícita `web/fs-promises` con `readdir` tipado para Dirent | `app/test/unit/hooks-discovery-test.ts` |
| streams de archivo escritos como builtin `fs` en el proxy de hooks | frontera explícita `web/fs`; stdin buffered usa WriteStream host-backed y lectura por rangos | `app/test/unit/traditional-hook-test.ts` |
| escritura/borrado temporal de reorder/squash como builtin `fs/promises` | frontera explícita `web/fs-promises`; las pruebas comparten un bridge Node mínimo de filesystem | `app/test/unit/git/reorder-test.ts`, `app/test/unit/git/squash-test.ts` |
| creación directa de directorios padre de locks de refs en update-ref | frontera explícita `web/fs-promises`; las suites usan el bridge Node de host compartido sin cambiar el orden de locking | `app/test/unit/git/assisted-commit-test.ts` |
| lanzamiento de terminales macOS mediante `child_process` | spawn host-backed `web/child_process`; la interfaz estructural se conserva porque el consumo es error/exit/stderr | `pnpm check:types`, `pnpm vite:build` |
| lanzamiento de terminales Windows mediante `child_process` y `shell: true` sin soporte host | spawn host-backed `web/child_process`; el host implementa unión de línea y `/bin/sh -c`/cmd según plataforma | `app/test/unit/cygwin-test.ts`, `cargo test -p desktop-shell-core-legacy io::process::tests::shell_commands_join_the_command_line_like_node` |
| ejecución directa del proxy de hooks y hooks tradicionales mediante `child_process` | frontera host-backed `web/child_process`; AbortSignal produce AbortError y kill host, pipe conserva `end: false`, y la lectura de stdin buffered respeta backpressure | `app/test/unit/traditional-hook-test.ts`, `app/test/unit/web-child-process-test.ts` |
| addon/vendor `windows-argv-parser` y shim web vacío | parsing Windows asíncrono en el host Rust mediante `CommandLineToArgvW`; el lanzamiento de shells personalizados espera ese parseo antes de spawn | `app/test/unit/native-custom-integration-test.ts`, `app/test/unit/backend-client-session-test.ts` |
| `stream/promises.pipeline` como rechazo permanente en el shim web | promesas sobre el pipeline/finished Node-compatible usado por el bundle; necesario para stdin de hooks | `app/test/unit/web-stream-promises-test.ts` |
| constructores vacíos `ReadStream`/`WriteStream` en el shim `stream` | eliminados por no tener consumidores; los streams de archivo viven en `web/fs` | `pnpm check:types`, `pnpm vite:build` |
| shim `app/src/web/web-utils.ts` sin consumidores | eliminado junto a su alias Vite; archivos soltados usan `hostFiles.getPathForFile` del host bridge | `pnpm check:types`, `pnpm vite:build` |
| loader legacy `app/test/globals.cjs` y subprocess del test de ciclo Git | eliminado; Vitest importa directamente el grafo ESM real y verifica identidad GitError | `app/test/unit/git-core-import-cycle-test.ts` |

## 2. Retirado como dependencia raíz, conservado temporalmente como fuente o shim

Estas piezas ya no son dependencias nativas del runtime Electron, pero sus
fuentes o shims siguen presentes porque todavía tienen consumidores migrando:

| Pieza | Estado actual | Condición de borrado final |
|---|---|---|
| `keytar`, `registry-js`, `fs-admin` | imports dirigidos a shims web o Rust | ningún consumidor ni test dependa del shim |
| `desktop-notifications` | fuentes vendorizadas/resolución de tipos para consumidores existentes | reemplazo Rust/tipado completo en notificaciones |
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

La línea base numérica viva está en `pnpm migration:status` (3 comprobaciones
`transportKind`, 18 llamadas legacy Git, 2 imports `dugite`, 8 imports `fs`, 3 imports `child_process`,
0 suites en el runner Node). `rust/ROADMAP.md` ordena el trabajo restante.

## 4. Qué se conserva

React/Primer, componentes, estilos, recursos vigentes, highlighter, submódulos
`gemoji`, `gitignore`, `choosealicense.com`, licencias, procedencia y oráculos
útiles. No se borran por parecer antiguos ni por ausencia en un import TS.
