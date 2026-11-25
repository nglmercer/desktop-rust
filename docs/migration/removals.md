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

La línea base numérica viva está en `pnpm migration:status` (181 comprobaciones
`transportKind`, 104 llamadas legacy Git, 22 imports `dugite`, 0 suites en el
runner Node a 2026-11-25). `rust/ROADMAP.md` ordena el trabajo restante.

## 4. Qué se conserva

React/Primer, componentes, estilos, recursos vigentes, highlighter, submódulos
`gemoji`, `gitignore`, `choosealicense.com`, licencias, procedencia y oráculos
útiles. No se borran por parecer antiguos ni por ausencia en un import TS.
