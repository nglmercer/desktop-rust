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
    "pnpm-workspace.yaml",
    "LICENSE",
    "app/package.json",
    "app/package-info.ts",
    "app/index.html",
    "app/src",
    "app/static",
    "app/styles",
    "gemoji/db/emoji.json",
    "gemoji/images/emoji",
    "script/licenses",
    "rust/crates/desktop-app/web/ipc-shim.ts",
];

/// The newest modification time under `path`, skipping `node_modules`.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    // A directory's mtime changes when an entry is removed. Looking only at
    // the remaining files misses deletions and leaves removed resources in
    // the embedded Vite bundle.
    let mut latest = meta.modified().ok();
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
        .any(|input| newest(&repo.join(input)).is_none_or(|time| time > built))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "desktop-vite-staleness-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            for input in INPUTS {
                let path = root.join(input);
                if matches!(
                    *input,
                    "app/src"
                        | "app/static"
                        | "app/styles"
                        | "gemoji/images/emoji"
                        | "script/licenses"
                ) {
                    fs::create_dir_all(path).unwrap();
                } else {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, b"fixture").unwrap();
                }
            }
            let dist = root.join("out/vite-dist");
            fs::create_dir_all(&dist).unwrap();
            for file in ["index.html", "highlighter.js", "ipc-shim.js"] {
                fs::write(dist.join(file), b"fixture").unwrap();
            }
            Self(root)
        }

        fn dist(&self) -> PathBuf {
            self.0.join("out/vite-dist")
        }

        fn set_index_time(&self, time: SystemTime) {
            set_modified(&self.dist().join("index.html"), time);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn set_modified(path: &Path, time: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(time))
            .unwrap();
    }

    #[test]
    fn resource_and_license_changes_invalidate_bundle() {
        let fixture = Fixture::new();
        let now = SystemTime::now();
        fixture.set_index_time(now + Duration::from_secs(60));
        assert!(!bundle_is_stale(&fixture.0, &fixture.dist()));
        for input in [
            "app/package.json",
            "app/package-info.ts",
            "gemoji/db/emoji.json",
            "LICENSE",
        ] {
            let path = fixture.0.join(input);
            set_modified(&path, now + Duration::from_secs(120));
            assert!(bundle_is_stale(&fixture.0, &fixture.dist()), "{input}");
            set_modified(&path, now);
        }
        for dir in ["app/static", "gemoji/images/emoji", "script/licenses"] {
            let path = fixture.0.join(dir).join("changed");
            fs::write(&path, b"fixture").unwrap();
            set_modified(&path, now + Duration::from_secs(120));
            assert!(bundle_is_stale(&fixture.0, &fixture.dist()), "{dir}");
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn removed_static_resource_invalidates_bundle() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app/static/obsolete.svg");
        fs::write(&path, b"fixture").unwrap();
        fixture.set_index_time(SystemTime::now());
        assert!(!bundle_is_stale(&fixture.0, &fixture.dist()));
        // Accommodate filesystems with coarse timestamp precision.
        std::thread::sleep(Duration::from_secs(2));
        fs::remove_file(path).unwrap();
        assert!(bundle_is_stale(&fixture.0, &fixture.dist()));
    }

    #[test]
    fn missing_input_or_generated_asset_invalidates_bundle() {
        let fixture = Fixture::new();
        fixture.set_index_time(SystemTime::now() + Duration::from_secs(60));
        assert!(!bundle_is_stale(&fixture.0, &fixture.dist()));
        fs::remove_file(fixture.0.join("app/package-info.ts")).unwrap();
        assert!(bundle_is_stale(&fixture.0, &fixture.dist()));

        let fixture = Fixture::new();
        fixture.set_index_time(SystemTime::now() + Duration::from_secs(60));
        fs::remove_file(fixture.dist().join("ipc-shim.js")).unwrap();
        assert!(bundle_is_stale(&fixture.0, &fixture.dist()));
    }
}
