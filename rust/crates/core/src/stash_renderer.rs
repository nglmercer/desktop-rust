//! Renderer-compatible stash file listings without a generic Git command API.
use crate::{Repository, args, process};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

impl Repository {
    /// Return Git's native raw/numstat stash display for a complete object ID.
    ///
    /// Keep `stash show` rather than substituting a commit diff: Git's stash
    /// configuration, rename detection, and optional untracked tree affect the
    /// output consumed by the existing Desktop parser.
    pub(crate) async fn renderer_stash_changes(&self, oid: &str) -> Result<Value> {
        ensure!(
            matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "A complete stash object ID is required"
        );
        let output = process::run_stash(
            &self.path,
            &args(&[
                "stash",
                "show",
                oid,
                "--raw",
                "--numstat",
                "-z",
                "--format=format:",
                "--no-show-signature",
                "--",
            ]),
        )
        .await?;
        ensure!(
            output.code == 0,
            "Cannot inspect stash: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(json!(String::from_utf8_lossy(&output.stdout)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("desktop-stash-renderer-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let fixture = Self(root);
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.name", "Stash test"]);
            fixture.git(&["config", "user.email", "stash@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fixture.git(&["config", "core.autocrlf", "false"]);
            fixture
        }
        fn git(&self, args: &[&str]) -> Vec<u8> {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        }
        fn show(&self, oid: &str) -> String {
            String::from_utf8_lossy(&self.git(&[
                "stash",
                "show",
                oid,
                "--raw",
                "--numstat",
                "-z",
                "--format=format:",
                "--no-show-signature",
                "--",
            ]))
            .into_owned()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn stash_changes_match_git_with_renames_binary_and_untracked_configuration() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("old name"), "a\nb\nc\n").unwrap();
        fs::write(fixture.0.join("binary"), [0, 1, 255, 0]).unwrap();
        fixture.git(&["add", "--all"]);
        fixture.git(&["commit", "-qm", "base"]);
        fixture.git(&["mv", "old name", "renamed ü"]);
        fs::write(fixture.0.join("binary"), [0, 2, 255, 0]).unwrap();
        fs::write(fixture.0.join("untracked file"), "untracked\n").unwrap();
        fixture.git(&["stash", "push", "-qu", "-m", "saved"]);
        let oid = String::from_utf8(fixture.git(&["rev-parse", "stash@{0}"]))
            .unwrap()
            .trim()
            .to_owned();
        let repo = Repository::open(&fixture.0).await.unwrap();
        for include in ["false", "true"] {
            fixture.git(&["config", "stash.showIncludeUntracked", include]);
            for renames in ["false", "true"] {
                fixture.git(&["config", "diff.renames", renames]);
                let expected = fixture.show(&oid);
                let actual = repo.renderer_stash_changes(&oid).await.unwrap();
                assert_eq!(actual, json!(expected));
                assert!(expected.contains("binary"));
                assert_eq!(expected.contains("untracked file"), include == "true");
            }
        }
    }

    #[tokio::test]
    async fn stash_changes_reject_options_revisions_and_missing_objects() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("file"), "base\n").unwrap();
        fixture.git(&["add", "file"]);
        fixture.git(&["commit", "-qm", "base"]);
        let repo = Repository::open(&fixture.0).await.unwrap();
        for invalid in [
            "HEAD",
            "--help",
            "",
            &"0".repeat(40),
            &"g".repeat(40),
            &"0".repeat(39),
        ] {
            assert!(
                repo.renderer_stash_changes(invalid).await.is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stash_changes_match_native_utf8_decoding_for_non_unicode_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let fixture = Fixture::new();
        let path = fixture.0.join(OsString::from_vec(b"invalid-\xff".to_vec()));
        fs::write(&path, "before\n").unwrap();
        fixture.git(&["add", "--all"]);
        fixture.git(&["commit", "-qm", "base"]);
        fs::write(&path, "after\n").unwrap();
        fixture.git(&["stash", "push", "-qm", "saved"]);
        let oid = String::from_utf8(fixture.git(&["rev-parse", "stash@{0}"]))
            .unwrap()
            .trim()
            .to_owned();
        let repo = Repository::open(&fixture.0).await.unwrap();
        assert_eq!(
            repo.renderer_stash_changes(&oid).await.unwrap(),
            json!(fixture.show(&oid))
        );
    }
}
