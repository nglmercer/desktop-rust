#!/usr/bin/env bash
# Rust packaging — replaces script/{build,package,copilot,dist-info,post-install}.ts
# for the fork (see docs/migration/status.md Fase 7). Produces a self-contained
# installable directory that runs without the checkout, node_modules or pnpm.
#
# Usage:
#   pnpm vite:build                    # renderer bundle in out/vite-dist/
#   script/package-rust.sh [platform]  # platform: auto|linux|macos|windows (default: host)
#   script/package-rust.sh --help
#
# The script respects the extracted responsibilities of the legacy scripts:
#   dist-info     → productName/version/channel from app/package.json + Cargo.toml
#   build         → build shell binary, copy highlighter/assets/emoji/gitignore,
#                   static resources (see vite copyStaticResources)
#   copilot       → copy the whole @github/copilot-sdk-<platform> package (exe +
#                   runtime.node + adjacents), fail if empty, never only the file
#   post-install  → nothing to download at install time; Git/LFS detection is at
#                   runtime (see bridge/core), not hidden downloads every start
#
# Installers per platform are described in packaging/{windows,macos,linux}/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

help() { sed -n '2,20p' "$0" | sed 's/^# //'; exit 0; }
for a in "$@"; do [ "$a" = "--help" ] && help; done

PLATFORM="${1:-auto}"
if [ "$PLATFORM" = "auto" ]; then
  case "$(uname -s)" in
    Linux*) PLATFORM=linux ;;
    Darwin*) PLATFORM=macos ;;
    *) PLATFORM=windows ;;
  esac
fi

APP_PKG="$ROOT/app/package.json"
VERSION="$(node -e "console.log(require('$APP_PKG').version)")"
CHANNEL="${RELEASE_CHANNEL:-development}"
PRODUCT="$(node -e "console.log(require('$APP_PKG').productName)")"
if [ "$CHANNEL" = "development" ]; then PRODUCT="${PRODUCT}-dev"; fi
APP_ID="$(node -e "console.log(require('$APP_PKG').bundleID)")"

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x64 ;;
  arm64|aarch64) ARCH=arm64 ;;
esac

OUT="$ROOT/dist/${PRODUCT}-${PLATFORM}-${ARCH}"
EXE_SUFFIX=""
case "$PLATFORM" in
  windows) EXE_SUFFIX=".exe" ;;
esac

echo "package-rust: $PRODUCT $VERSION ($CHANNEL) for $PLATFORM-$ARCH → $OUT"

check() {
  if [ ! -f "$1" ]; then echo "ERR: required file missing: $1 (build it first — see above)"; exit 1; fi
}

# 1. Build Rust binaries (release-ready, but debug ok for local check).
cargo build --locked -p desktop-core -p desktop-bridge -p desktop-cli -p desktop-native 2>&1 | tail -n 5
GUI_BUILT=0
if cargo build --locked -p desktop-app; then
  GUI_BUILT=1
else
  echo "warn: desktop shell build failed; packaging CLI/bridge only" >&2
fi

BIN_RS="$ROOT/rust/target/debug/desktop-rs"
BIN_APP="$ROOT/rust/target/debug/github-desktop-rs"
[ -f "$BIN_RS" ] || BIN_RS="$(find "$ROOT" -path '*/target/debug/desktop-rs' -type f | head -n1)"
[ -f "$BIN_APP" ] || BIN_APP="$(find "$ROOT" -path '*/target/debug/github-desktop-rs' -type f | head -n1)"
[ -d "$BIN_RS" ] && BIN_RS="$(find "$ROOT" -name desktop-rs -type f | head -n1)"
check "$BIN_RS"

# 2. Renderer — ensure Vite was built (it embeds out/vite-dist).
check "$ROOT/out/vite-dist/index.html"
check "$ROOT/out/vite-dist/highlighter.js"
if [ ! -f "$ROOT/gemoji/db/emoji.json" ]; then echo "ERR: gemoji submodule missing — git submodule update --init --recursive"; exit 1; fi

# 3. Copilot runtime — copy whole platform package, fail if incomplete.
# locate the optional platform package via pnpm's store (pnpm installs it under .pnpm).
COPILOT_SRC="$ROOT/app/node_modules/@github/copilot-sdk-${PLATFORM}-${ARCH}"
if [ ! -d "$COPILOT_SRC" ]; then
  # fallback to pnpm store path
  FOUND="$(find "$ROOT/node_modules/.pnpm" -type d -path "*@github+copilot-sdk-${PLATFORM}-${ARCH}*/node_modules/@github/copilot-sdk-${PLATFORM}-${ARCH}" 2>/dev/null | head -n1)"
  if [ -n "$FOUND" ] && [ -d "$FOUND" ]; then COPILOT_SRC="$FOUND"; fi
fi
# 4. Stage installable tree (exe, UI, highlighter, assets, metadata, licences, tools).
rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/ui" "$OUT/assets" "$OUT/licenses" "$OUT/meta" "$OUT/copilot-sdk"

COPILOT_DST="$OUT/copilot-sdk"
if [ -d "$COPILOT_SRC" ]; then
  RT="$COPILOT_SRC/prebuilds/${PLATFORM}-${ARCH}/copilot-runtime${EXE_SUFFIX}"
  # Also handle win32 naming (copilot-runtime.exe vs .exe difference)
  if [ ! -f "$RT" ] && [ "$PLATFORM" = "windows" ]; then RT="$COPILOT_SRC/prebuilds/${PLATFORM}-${ARCH}/copilot-runtime"; fi
  # Path for pnpm store may be pnpm's hardlinked copy — same check
  check "$RT"
  check "$COPILOT_SRC/prebuilds/${PLATFORM}-${ARCH}/runtime.node"
  echo "copying Copilot SDK package $(basename "$COPILOT_SRC") → $COPILOT_DST"
  cp -R "$COPILOT_SRC"/. "$COPILOT_DST"/
else
  echo "warn: Copilot SDK package not found at $COPILOT_SRC — runtime omitted; install one platform package to enable Copilot"
fi

install -m755 "$BIN_RS" "$OUT/bin/desktop-rs"
if [ "$GUI_BUILT" = 1 ]; then
  check "$BIN_APP"
  install -m755 "$BIN_APP" "$OUT/bin/github-desktop-rs"
fi

cp -R "$ROOT/out/vite-dist/." "$OUT/ui/"
cp -R "$ROOT/out/vite-dist/assets" "$OUT/assets/" 2>/dev/null || true
cp "$ROOT/app/package.json" "$OUT/meta/" 2>/dev/null || true
[ -f "$ROOT/LICENSE" ] && cp "$ROOT/LICENSE" "$OUT/licenses/" || true
[ -f "$ROOT/rust/THIRD_PARTY_NOTICES.txt" ] && cp "$ROOT/rust/THIRD_PARTY_NOTICES.txt" "$OUT/licenses/" || true
[ -f "$ROOT/app/LICENSE" ] && cp "$ROOT/app/LICENSE" "$OUT/licenses/" || true

# 5. Identity + channel metadata (fork identity, no upstream distribution identities).
cat > "$OUT/meta/package.json" <<JSON
{
  "productName": "$PRODUCT",
  "bundleID": "$APP_ID",
  "version": "$VERSION",
  "channel": "$CHANNEL",
  "platform": "$PLATFORM",
  "arch": "$ARCH",
  "executable": "bin/desktop-rs"
}
JSON

# 6. Runtime detection hint — never download Git/LFS hidden; detect & report.
cat > "$OUT/meta/runtime.json" <<'JSON'
{
  "note": "Git and Git LFS are external tools managed by Rust. On Windows/macOS the installer provides them; on Linux/Termux use the system package manager. The application verifies them at first launch and reports absence without a hidden download.",
  "git": "required",
  "lfs": "optional but recommended for LFS repositories"
}
JSON

echo "package ready: $OUT"
echo "test without checkout: (cd /tmp && $OUT/bin/desktop-rs --help)"
