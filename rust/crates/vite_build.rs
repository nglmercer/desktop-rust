use std::path::{Path, PathBuf};

fn checked_path(path: &Path) -> &str {
    let text = path.to_str().expect("renderer bundle path must be UTF-8");
    assert!(
        !text.contains(['\0', '\r', '\n']),
        "renderer bundle path contains a control character"
    );
    text
}

/// Resolve a path even when its trailing components have not been created yet.
fn canonical_with_missing(path: &Path) -> PathBuf {
    let mut current = path;
    let mut missing = Vec::new();
    loop {
        match current.canonicalize() {
            Ok(mut base) => {
                for component in missing.iter().rev() {
                    base.push(component);
                }
                return base;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    current
                        .file_name()
                        .expect("missing path has no name")
                        .to_owned(),
                );
                current = current.parent().expect("missing path has no parent");
            }
            Err(error) => panic!("cannot resolve development bundle path: {error}"),
        }
    }
}

/// Resolve an already built bundle, refusing aliases of the live watcher output.
pub fn explicit_bundle(repo: &Path, value: std::ffi::OsString) -> PathBuf {
    let text = value.to_str().expect("DESKTOP_VITE_DIST must be UTF-8");
    assert!(
        !text.contains(['\0', '\r', '\n']),
        "DESKTOP_VITE_DIST contains a control character"
    );
    let path = PathBuf::from(text);
    assert!(path.is_absolute(), "DESKTOP_VITE_DIST must be absolute");
    let path = path
        .canonicalize()
        .expect("DESKTOP_VITE_DIST must be an existing directory");
    checked_path(&path);
    assert!(path.is_dir(), "DESKTOP_VITE_DIST must be a directory");
    let live = canonical_with_missing(&repo.join("out/vite-dist"));
    assert!(
        !path.starts_with(&live) && !live.starts_with(&path),
        "DESKTOP_VITE_DIST must not overlap the live development bundle"
    );
    for asset in ["index.html", "highlighter.js", "ipc-shim.js"] {
        assert!(
            path.join(asset).is_file(),
            "DESKTOP_VITE_DIST is missing {asset}"
        );
    }
    path
}

pub fn export_bundle(dist: &Path) {
    let dist = checked_path(dist);
    println!("cargo:rerun-if-env-changed=DESKTOP_VITE_DIST");
    println!("cargo:rerun-if-changed={dist}");
    for asset in ["index.html", "highlighter.js", "ipc-shim.js"] {
        println!(
            "cargo:rerun-if-changed={}",
            Path::new(dist).join(asset).display()
        );
    }
    println!("cargo:rustc-env=DESKTOP_EMBEDDED_UI_DIR={dist}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "desktop-vite-resolve-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn assets(&self, path: &Path) {
            fs::create_dir_all(path).unwrap();
            for file in ["index.html", "highlighter.js", "ipc-shim.js"] {
                fs::write(path.join(file), "fixture").unwrap();
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn rejects_live_ancestor_even_before_live_directory_exists() {
        let fixture = Fixture::new();
        let out = fixture.0.join("out");
        fixture.assets(&out);
        assert!(
            std::panic::catch_unwind(|| explicit_bundle(&fixture.0, out.into_os_string())).is_err()
        );
        let isolated = fixture.0.join("isolated");
        fixture.assets(&isolated);
        assert_eq!(
            explicit_bundle(&fixture.0, isolated.clone().into_os_string()),
            isolated.canonicalize().unwrap()
        );
    }
    #[cfg(unix)]
    #[test]
    fn rejects_control_characters_and_non_utf8_in_canonical_path() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        for name in [
            std::ffi::OsString::from("bad\nname"),
            std::ffi::OsString::from_vec(vec![b'b', 0xff]),
        ] {
            let target = fixture.0.join(name);
            fixture.assets(&target);
            let alias = fixture.0.join("alias");
            symlink(target, &alias).unwrap();
            assert!(
                std::panic::catch_unwind(|| explicit_bundle(
                    &fixture.0,
                    alias.clone().into_os_string()
                ))
                .is_err()
            );
            fs::remove_file(alias).unwrap();
        }
    }
}
