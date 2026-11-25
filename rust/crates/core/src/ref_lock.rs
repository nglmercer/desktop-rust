//! Read the parent-owned ref-lock context needed by assisted-commit transactions.
use crate::{Repository, args};
use anyhow::Result;
use serde_json::{Value, json};

fn line(output: &[u8], what: &str, path: bool) -> Result<String> {
    anyhow::ensure!(output.len() <= 65536, "{what} exceeds limit");
    // The previous renderer decoded process output as UTF-8; keep that lossy
    // behavior rather than rejecting a valid non-UTF-8 repository path.
    let text = String::from_utf8_lossy(output);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    anyhow::ensure!(
        !text.is_empty() && !text.contains('\0') && (path || !text.contains(['\n', '\r'])),
        "invalid {what}"
    );
    Ok(text.to_owned())
}

fn valid_expected(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Repository {
    /// Read, in one repository operation, the file-backed lock paths and whether
    /// the requested ref still has the caller's expected value.
    pub(crate) async fn ref_lock_context(
        &self,
        reference: &str,
        expected_value: &str,
    ) -> Result<Value> {
        anyhow::ensure!(
            reference == "HEAD" || reference.starts_with("refs/heads/"),
            "Parent-owned ref locks require HEAD or a fully qualified branch"
        );
        anyhow::ensure!(
            reference.len() <= 4096 && !reference.contains(['\0', '\n', '\r']),
            "Invalid ref lock reference"
        );
        anyhow::ensure!(valid_expected(expected_value), "Invalid expected object ID");
        let format = self.run(&args(&["rev-parse", "--show-ref-format"])).await?;
        anyhow::ensure!(format.code == 0, "Cannot read ref storage format");
        anyhow::ensure!(
            line(&format.stdout, "ref storage format", false)? == "files",
            "Parent-owned ref locks require file-backed Git refs"
        );
        let names = if reference == "HEAD" {
            vec!["HEAD"]
        } else {
            vec!["HEAD", reference]
        };
        let mut paths = Vec::with_capacity(names.len());
        for name in names {
            let output = self
                .run(&args(&[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    name,
                ]))
                .await?;
            anyhow::ensure!(output.code == 0, "Cannot resolve ref lock path");
            let path = line(&output.stdout, "ref lock path", true)?;
            anyhow::ensure!(
                std::path::Path::new(&path).is_absolute(),
                "Ref lock path must be absolute"
            );
            paths.push(path);
        }
        let current = self
            .run(&args(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ]))
            .await?;
        anyhow::ensure!(
            matches!(current.code, 0 | 1),
            "Cannot read locked ref value"
        );
        let actual = if current.code == 0 {
            line(&current.stdout, "locked ref value", false)?
        } else {
            "0".repeat(expected_value.len())
        };
        Ok(json!({
            "paths": paths,
            "matches": actual == expected_value,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::valid_expected;
    use crate::Repository;
    use std::{fs, process::Command};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let name = if cfg!(windows) {
                format!("desktop-ref-lock-{}", uuid::Uuid::new_v4())
            } else {
                format!("desktop-ref-lock-\"quoted\"\n{}", uuid::Uuid::new_v4())
            };
            let root = std::env::temp_dir().join(name);
            fs::create_dir(&root).unwrap();
            let fixture = Self(root);
            fixture.git(&["init", "-q", "-b", "main"]);
            fixture.git(&["config", "user.name", "Test"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fs::write(fixture.0.join("file"), "commit\n").unwrap();
            fixture.git(&["add", "file"]);
            fixture.git(&["commit", "-qm", "commit"]);
            fixture
        }

        fn git(&self, args: &[&str]) -> String {
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
            String::from_utf8(output.stdout).unwrap().trim().into()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn expected_values_are_bounded_hex_object_ids() {
        assert!(valid_expected("0123456789abcdefABCDEF"));
        assert!(!valid_expected(""));
        assert!(!valid_expected("not-an-object-id"));
        assert!(!valid_expected(&"0".repeat(129)));
    }

    #[tokio::test]
    async fn context_reports_head_and_branch_lock_paths_and_expected_value() {
        let fixture = Fixture::new();
        let head = fixture.git(&["rev-parse", "HEAD"]);
        let repository = Repository::open(&fixture.0).await.unwrap();
        let context = repository
            .ref_lock_context("refs/heads/main", &head)
            .await
            .unwrap();
        assert_eq!(context["matches"], true);
        let paths = context["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].as_str().unwrap().ends_with("/.git/HEAD"));
        assert!(
            paths[1]
                .as_str()
                .unwrap()
                .ends_with("/.git/refs/heads/main")
        );
    }

    #[tokio::test]
    async fn context_distinguishes_mismatch_missing_ref_and_detached_head() {
        let fixture = Fixture::new();
        let head = fixture.git(&["rev-parse", "HEAD"]);
        let zero = "0".repeat(head.len());
        let repository = Repository::open(&fixture.0).await.unwrap();
        assert_eq!(
            repository
                .ref_lock_context("refs/heads/main", &zero)
                .await
                .unwrap()["matches"],
            false
        );
        assert_eq!(
            repository
                .ref_lock_context("refs/heads/missing", &zero)
                .await
                .unwrap()["matches"],
            true
        );
        fixture.git(&["switch", "-q", "--detach", "HEAD"]);
        let detached = repository.ref_lock_context("HEAD", &head).await.unwrap();
        assert_eq!(detached["matches"], true);
        assert_eq!(detached["paths"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn context_rejects_non_heads_and_invalid_expected_values() {
        let fixture = Fixture::new();
        let head = fixture.git(&["rev-parse", "HEAD"]);
        let repository = Repository::open(&fixture.0).await.unwrap();
        assert!(
            repository
                .ref_lock_context("refs/remotes/origin/main", &head)
                .await
                .is_err()
        );
        assert!(
            repository
                .ref_lock_context("HEAD", "not-an-object-id")
                .await
                .is_err()
        );
    }
}
