//! Makes sure the renderer bundle that `desktop-app` embeds is current.
//!
//! The bundle is produced by one command, `pnpm vite:build` (the app and the
//! highlighter worker and Wry bootstrap). This script runs it when the bundle is missing, when
//! any input is newer than the bundle, or when `DESKTOP_VITE_FORCE` is set.
//! Existence of `index.html` and `highlighter.js` alone does not make a bundle
//! current.
//!
//! There is no fallback to another package manager: if the build fails, the
//! Cargo build stops with the cause.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Inputs whose change makes the bundle stale. Directories are scanned.
const INPUTS: &[&str] = &[
    "vite.config.ts",
    "vite.highlighter.config.ts",
    "vite.ipc-shim.config.ts",
    "package.json",
    "pnpm-lock.yaml",
    "app/index.html",
    "app/src",
    "app/styles",
    "rust/crates/desktop-app/web/ipc-shim.ts",
];

/// The newest modification time under `path`, skipping `node_modules`.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    let mut latest = None;
    for entry in std::fs::read_dir(path).ok()?.flatten() {
        if entry.file_name() == "node_modules" {
            continue;
        }
        if let Some(time) = newest(&entry.path()) {
            latest = Some(latest.map_or(time, |current: SystemTime| current.max(time)));
        }
    }
    latest
}

fn bundle_is_stale(repo: &Path, dist: &Path) -> bool {
    if !dist.join("highlighter.js").is_file() || !dist.join("ipc-shim.js").is_file() {
        return true;
    }
    let Ok(built) = std::fs::metadata(dist.join("index.html")).and_then(|m| m.modified()) else {
        return true;
    };
    INPUTS
        .iter()
        .filter_map(|input| newest(&repo.join(input)))
        .any(|input| input > built)
}

fn pnpm() -> &'static str {
    if cfg!(windows) { "pnpm.cmd" } else { "pnpm" }
}

fn build_bundle(repo: &Path, dist: &Path) -> Result<(), String> {
    let probe = Command::new(pnpm())
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "pnpm was not found ({error}). Install the version named by `packageManager` in package.json, then run `pnpm install --frozen-lockfile --ignore-scripts` from the repository root."
            )
        })?;
    if !probe.status.success() {
        return Err("`pnpm --version` failed; check the pnpm installation".into());
    }

    println!("cargo:warning=building the renderer bundle with `pnpm vite:build`");
    let status = Command::new(pnpm())
        .arg("vite:build")
        .current_dir(repo)
        .status()
        .map_err(|error| format!("could not start `pnpm vite:build`: {error}"))?;
    if !status.success() {
        return Err(format!(
            "`pnpm vite:build` failed ({status}). The Vite error printed above is the cause. Fix it, or run `pnpm vite:build` on its own to see it again, then rebuild."
        ));
    }
    if !dist.join("index.html").is_file()
        || !dist.join("highlighter.js").is_file()
        || !dist.join("ipc-shim.js").is_file()
    {
        return Err(format!(
            "`pnpm vite:build` finished, but {} is incomplete",
            dist.display()
        ));
    }
    Ok(())
}

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo: PathBuf = manifest.join("../../..");
    let dist = repo.join("out/vite-dist");

    // Cargo re-runs this script when an input changes, and this check decides
    // whether anything has to be rebuilt.
    for input in INPUTS {
        println!("cargo:rerun-if-changed={}", repo.join(input).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("ipc-shim.js").display()
    );
    println!("cargo:rerun-if-env-changed=DESKTOP_VITE_FORCE");
    println!("cargo:rerun-if-env-changed=DESKTOP_SERVE_FROM_DISK");

    let force = std::env::var_os("DESKTOP_VITE_FORCE").is_some();
    if force || bundle_is_stale(&repo, &dist) {
        if let Err(cause) = build_bundle(&repo, &dist) {
            panic!("the renderer bundle could not be built: {cause}");
        }
    }
}
