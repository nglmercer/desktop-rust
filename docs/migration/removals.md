# Retiradas — desktop-rust

> Fase 0 — inventario de candidatos. Cada entrada: ruta → responsabilidad → reemplazo → consumidores revisados → compatibilidad → prueba → commit de retirada.
> No borra todavía; la Fase final ejecuta la eliminación después de conectar y verificar.

## 1. Registro por ruta

```text
Ruta retirada: (pendiente) rust/crates/desktop-core  [renombrado a desktop-shell-core-legacy]
Responsabilidad anterior: dominio desktop GUI-free: models, git, storage, api, state, ipc, platform, io/limits/executor/process
Reemplazo conectado: rust/crates/core (único desktop-core definitivo) + rust/crates/desktop-app (Tao/Wry)
Consumidores, imports dinámicos y scripts revisados: rust/crates/desktop-app/Cargo.toml (antes desktop-core), Cargo.toml raíz workspace.dependencies, todo el crate desktop-app/src/*.rs (DesktopApp, io::executor/limits/lane, ipc::channels/protocol)
Datos o formatos que deben seguir siendo compatibles: state JSON sanitizado sin tokens, SecureStore DPAPI, AppPaths, persistencia atómica
Prueba o recorrido que acredita la sustitución: cargo test --workspace (raíz) + app_smoke.rs + suites rust/tests/*.py
Commit de retirada: (pendiente Fase final)
```

```text
Ruta retirada: rust/Cargo.toml + rust/Cargo.lock (workspace shell)
Responsabilidad anterior: workspace shell separado (desktop-core legacy + desktop-app)
Reemplazo conectado: Cargo.toml raíz consolidado (un solo workspace, un solo lock)
Consumidores, imports dinámicos y scripts revisados: .github/workflows/rust.yml, script/package-rust.ts, MIGRATION.md, rust/README.md
Datos o formatos que deben seguir siendo compatibles: MSRV 1.85, edition, features, perfiles, dependencias heredadas propias del shell
Prueba o recorrido que acredita la sustitución: cargo metadata (raíz) single workspace; build CLI/core/bridge sin GTK via default-members; shell build explícito en CI
Commit de retirada: (pendiente)
```

```text
Ruta retirada: (pendiente) rust/crates/native como host independiente
Responsabilidad anterior: host Wry/Tao alternativo que sirve assets del bridge (desktop-native)
Reemplazo conectado: rust/crates/desktop-app (github-desktop-rs, Tao/Wry + desktop://) + rust/crates/cli (desktop-rs serve) para Termux/browser
Consumidores, imports dinámicos y scripts revisados: rust/crates/native/tests/{smoke_launch,headless_cli}.rs, .github/workflows/rust.yml (job native), rust/README.md
Datos o formatos que deben seguir siendo compatibles: argumentos (--empty/--selected/-C/--config/--smoke-test) y códigos de retorno vía wrapper de compatibilidad
Prueba o recorrido que acredita la sustitución: native job en CI (xvfb smoke) + wrapper de compatibilidad verificado
Commit de retirada: (pendiente Fase final; conservar wrapper fino si hay consumidores del ejecutable anterior)
```

```text
Ruta retirada: app/src/main-process/** + entradas Electron + crash shell
Responsabilidad anterior: main process Electron (ventana, menú, dialogs, updater, crash)
Reemplazo conectado: rust/crates/desktop-app/src/{main,host,instance,menu,protocol}.rs (Tao/Wry, instancia única, protocolos)
Consumidores, imports dinámicos y scripts revisados: app/src/** imports electron, shims app/src/web/electron*.ts, vite.config.ts (ya no usa webpack), package.json (electron/packager)
Datos o formatos que deben seguir siendo compatibles: identidad fork propia, directorios propios (no colisión con desktop oficial), protocolo x-github-client://
Prueba o recorrido que acredita la sustitución: app_smoke.rs (arranque real), prueba GUI Windows/macOS/Linux
Commit de retirada: (pendiente)
```

```text
Ruta retirada: app/webpack.* + loaders + postinstall Electron
Responsabilidad anterior: build Electron (webpack), post-install downloads
Reemplazo conectado: vite.config.ts + vite.highlighter.config.ts + pnpm-lock.yaml + script/package-rust.*
Consumidores, imports dinámicos y scripts revisados: package.json scripts (compile:dev/prod, build, postinstall), script/build.ts, CI
Datos o formatos que deben seguir siendo compatibles: out/vite-dist (index.html, assets, highlighter.js, emoji) consumido por bridge y desktop-app
Prueba o recorrido que acredita la sustitución: pnpm vite:build + cargo build embedding + CI sin --ignore-scripts como dependencia permanente
Commit de retirada: (pendiente Fase final)
```

```text
Ruta retirada: script/{build,package,copilot,dist-info,post-install}.ts + scripts antiguos de release
Responsabilidad anterior: packaging, copia Copilot, firma, distribución
Reemplazo conectado: script/package-rust.* + packaging/{windows,macos,linux}/
Consumidores, imports dinámicos y scripts revisados: package.json (package, start, draft-release), .github/workflows/*, script/copilot.ts (validación runtime.node)
Datos o formatos que deben seguir siendo compatibles: runtime Copilot completo (exe + runtime.node + adyacentes), licencias, Git/LFS por plataforma, metadatos fork
Prueba o recorrido que acredita la sustitución: paquete limpio sin checkout/node_modules, instalación sin Electron
Commit de retirada: (pendiente)
```

```text
Ruta retirada: Electron, electron-packager, electron-builder, devtools y dependencias exclusivas (package.json + package-lock.json npm)
Responsabilidad anterior: runtime y build Electron
Reemplazo conectado: Tao/Wry + Rust toolchain + pnpm/Vite
Consumidores, imports dinámicos y scripts revisados: app/package.json, package.json (electron version), .eslintrc, script/*.ts, tests E2E playwright._electron
Datos o formatos que deben seguir siendo compatibles: none (reemplazo binario nativo)
Prueba o recorrido que acredita la sustitución: cargo build + pnpm vite:build + instalación limpia sin descarga Electron
Commit de retirada: (pendiente)
```

```text
Ruta retirada: SDK Copilot en renderer (@github/copilot-sdk) + adaptadores de procesos TS
Responsabilidad anterior: sesiones Copilot y assisted-commit (copilot-store.ts, copilot-operation.ts, copilot-runtime.ts, lib/git/assisted-commit/*)
Reemplazo conectado: cliente Rust acotado en core con protocolo real del runtime
Consumidores, imports dinámicos y scripts revisados: app/src/lib/copilot*.ts, app/src/lib/stores/copilot-store.ts, lib/git/assisted-commit/*
Datos o formatos que deben seguir siendo compatibles: snapshot/captura/valida, lease de transacción, frontera push asistido (rollback solo antes de push nativo)
Prueba o recorrido que acredita la sustitución: crear sesión + completar acción desde UI, cancelación/recuperación, runtime distribuido verificado
Commit de retirada: (pendiente Fase 5)
```

```text
Ruta retirada: shims app/src/web/* (individuales), aliases, polyfills
Responsabilidad anterior: child_process, fs, keytar, dugite, url, crypto, electron-window-state, etc. sobre Node/Electron
Reemplazo conectado: app/src/backend/client.ts + transportes Wry/WebSocket; utilidades puras web quedan
Consumidores, imports dinámicos y scripts revisados: rg imports directos/transitivos/dinámicos en app/src/{ui,lib,stores}/**; vite.config.ts ya alias those shims
Datos o formatos que deben seguir siendo compatibles: utilidades web puras (path/url/stream bufferizados) que siguen usándose
Prueba o recorrido que acredita la sustitución: pnpm vite:build sin Node shims; tests web-* migrados a su equivalente Rust
Commit de retirada: (pendiente, revisión individual por archivo)
```

```text
Ruta retirada: rust/crates/desktop-app/web/ipc-shim.js (cuando Wry ya tenga bootstrap tipado)
Responsabilidad anterior: shim ipcRenderer sobre window.ipc.postMessage + evaluate_script
Reemplazo conectado: app/src/backend/client.ts (tipado, <50 ms primeros bytes, `read_skill table-fit` no aplica aquí)
Consumidores, imports dinámicos y scripts revisados: desktop-app/src/{bridge,host,main}.rs INIT_SCRIPT, build.rs (solo ipc-shim.js)
Datos o formatos que deben seguir siendo compatibles: wire format __rpc/__invoke (version 3, capabilities cancel/max_pending) si aún lo usa Wry
Prueba o recorrido que acredita la sustitución: desktop-app smoke sin shim, transporte tipado verificado
Commit de retirada: (pendiente — si aún lo usa Wry, reemplazar antes)
```

```text
Ruta retirada: dugite, keytar, trampolines, módulos nativos (fs-admin, koffi, registry-js)
Responsabilidad anterior: git exec, keychain, helpers de plataforma
Reemplazo conectado: Rust: git via process runner, SecretStore nativo (DPAPI/Keychain/Secret Service), trampolines via crate dedicated
Consumidores, imports dinámicos y scripts revisados: app/src/lib/git/*, app/src/web/{dugite,keytar,registry-js,fs-admin,koffi}.ts, app/src/lib/api.ts
Datos o formatos que deben seguir siendo compatibles: credenciales migradas solo tras comprobar destino; sin tokens en logs/URLs
Prueba o recorrido que acredita la sustitución: suites Rust y Vitest de credenciales, GitHub y descarte.
Commit de retirada: (pendiente)
```

```text
Ruta retirada: vendor/desktop-notifications, vendor/desktop-trampoline, printenvz, windows-argv-parser
Responsabilidad anterior: binarios helpers Windows
Reemplazo conectado: Rust nativo (notificaciones, argv, trampolines)
Consumidores, imports dinámicos y scripts revisados: app/package.json deps "file:../vendor/*", app/src/web/{desktop-notifications,desktop-trampoline,windows-argv-parser}.ts
Datos o formatos que deben seguir siendo compatibles: argv quoting, notificaciones, Unicode
Prueba o recorrido que acredita la sustitución: Windows x64 CI, rutas Unicode/espacios
Commit de retirada: (pendiente, verificación individual)
```

```text
Ruta retirada: lockfiles npm (package-lock.json) + scripts npm exclusivos
Responsabilidad anterior: install lock npm universal
Reemplazo conectado: pnpm-lock.yaml (actual) + Cargo.lock (único a final)
Consumidores, imports dinámicos y scripts revisados: package.json (postinstall, clean-slate, npm scripts), scripts/*, CI
Datos o formatos que deben seguir siendo compatibles: pnpm-lock.yaml actual; fixtures/tools independientes conservan sus lockfiles si se necesitan
Prueba o recorrido que acredita la sustitución: pnpm install --frozen-lockfile + cargo build --locked limpio
Commit de retirada: (pendiente Fase final)
```

```text
Ruta retirada: Tests/fixtures/documentation anteriores exclusivas de rutas eliminadas
Responsabilidad anterior: suites Electron, fixtures obsoletas, docs Electron
Reemplazo conectado: suites rust/tests/*.py (41 headless), app_smoke.rs (GUI), docs/migration/*, rust/{README,MIGRATION}.md, MIGRATION.md actualizado
Consumidores, imports dinámicos y scripts revisados: app/test/unit/** Electron, script/*.ts, docs/**, README.md, AGENTS.md, CONTRIBUTING
Datos o formatos que deben seguir siendo compatibles: oráculos branch/lfs (branch-rules-oracle, lfs-attributes-oracle.json), licencias, procedencia (THIRD_PARTY_NOTICES), gemoji/gitignore/choosealicense submódulos
Prueba o recorrido que acredita la sustitución: suites Cargo y Vitest + vite build + app_smoke + build limpio con submódulos
Commit de retirada: (pendiente — conservar React/Primer, estilos, componentes, recursos vigentes, highlighter, submódulos, licencias, oráculos)
```

## 2. Candidatos y condiciones (tabla ejecutiva)

| Candidato | Condición antes de eliminar |
|---|---|
| `rust/crates/desktop-core` legacy → `desktop-shell-core-legacy` (renombrado Fase 2) | Todos los servicios útiles integrados al `core` definitivo y GUI/CLI/bridge usan `desktop-core` |
| `rust/Cargo.toml`/`lock` | Retirados; workspace raíz consolidado y comandos migrados |
| `rust/crates/native` host independiente | Shell definitivo probado; wrapper compatibilidad solo si quedan consumidores |
| Dominio TS en stores/`lib/git` | No decide efectos ni persistencia; conserva interfaces/proyecciones |
| `app/src/main-process/**`, entradas Electron, crash shell | Responsabilidades portadas, referencias resueltas, plataforma verificada |
| `app/webpack.*`, loaders Electron | Vite cubre UI, workers, highlighter, recursos, dev |
| Scripts build/package/postinstall/release antiguos | Recursos/Copilot/licencias/firma/distribución trasladados y verificados |
| Electron/packager/installer/devtools/deps exclusivas | Sin consumidores runtime/tests/scripts/tipos/build; instalación limpia sin ellos |
| SDK Copilot renderer + adaptadores | Cliente Rust con paridad; runtime externo en paquete |
| Shims `app/src/web/*`, aliases, polyfills | Revisión individual; conservar utilidades web puras |
| `ipc-shim.js` | Bootstrap tipado definitivo; si aún usa Wry, reemplazar antes |
| `dugite`/`keytar`/trampolines/módulos nativos | Operaciones/parsers/credenciales/persistencia migradas |
| `vendor/desktop-*`, `printenvz`, `windows-argv-parser` | Verificación individual por función |
| Lockfiles npm y scripts npm exclusivos | Consumidores migrados; conservar fixtures/tooling independientes |
| Tests/fixtures/docs anteriores | Casos trasladados; retirar solo exclusivos de rutas eliminadas |

## 3. Qué se conserva

React/Primer, componentes, estilos, recursos vigentes, highlighter, submódulos `gemoji`, `gitignore`, `choosealicense.com`, licencias, procedencia y oráculos útiles. No se borran por parecer antiguos ni por ausencia en un import TS.
Los tres archivos UI de `desktop-app/web` (`app.js`, `index.html`, `styles.css`) ya se eliminaron en la base auditada `ca6acec28f` — no se restauran ni cuentan como trabajo nuevo.

## 4. Fase final

La limpieza se hace en commits identificables. Empieza tras conectar reemplazos y superar verificaciones. Consta de:
1) Consolidar workspace único + single `Cargo.lock` (`cargo metadata` limpia);
2) Reconstruir (headless + shell);
3) Eliminar legacy con registro en esta lista (con commit SHA);
4) `git status` limpio sin collateral, referencias/manifiestos/scripts/workflows/docs actualizados;
5) Instalación desde checkout limpio con submódulos, sin artefactos, sin descarga Electron.
