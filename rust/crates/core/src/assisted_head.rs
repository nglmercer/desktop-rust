//! Read the HEAD identity needed by assisted-commit snapshot guards.
use crate::{Repository, args};
use anyhow::Result;
use serde_json::{Value, json};

fn output_line(output: &[u8], what: &str, path: bool) -> Result<String> {
    anyhow::ensure!(output.len() <= 65536, "{what} exceeds limit");
    let text = String::from_utf8_lossy(output);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    anyhow::ensure!(
        !text.is_empty() && !text.contains('\0') && (path || !text.contains(['\n', '\r'])),
        "invalid {what}"
    );
    Ok(text.to_owned())
}

fn invalid(message: impl Into<String>) -> Value {
    json!({"kind":"invalid", "message":message.into()})
}

impl Repository {
    pub(crate) async fn assisted_commit_safety(&self) -> Result<Value> {
        let replacements = self.run(&args(&["replace", "--list"])).await?;
        anyhow::ensure!(replacements.code == 0, "Cannot inspect replacement refs");
        if !replacements.stdout.is_empty() {
            return Ok(invalid(
                "Assisted commits do not support replacement refs or grafted history",
            ));
        }
        let directory = self.git_directory().await?;
        let grafts = std::env::var_os("GIT_GRAFT_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| directory.join("info/grafts"));
        match std::fs::symlink_metadata(&grafts) {
            Ok(_) => {
                return Ok(invalid(
                    "Assisted commits do not support replacement refs or grafted history",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for name in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
            "BISECT_LOG",
        ] {
            match std::fs::symlink_metadata(directory.join(name)) {
                Ok(_) => {
                    return Ok(invalid(format!(
                        "Cannot create assisted commits during a Git operation ({name})"
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let conflicts = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(
            conflicts.code == 0,
            "Cannot inspect unresolved index entries"
        );
        if !conflicts.stdout.is_empty() {
            return Ok(invalid("The real index contains unresolved conflicts"));
        }
        Ok(json!({"kind":"ok"}))
    }

    /// Read HEAD, its optional branch, tip, and absolute Git directory in one
    /// repository operation. Expected unsafe states are data, not transport errors.
    pub(crate) async fn assisted_commit_head(&self) -> Result<Value> {
        let directory = self
            .run(&args(&["rev-parse", "--absolute-git-dir"]))
            .await?;
        anyhow::ensure!(directory.code == 0, "Cannot read the Git directory");
        let git_dir = output_line(&directory.stdout, "Git directory", true)?;
        anyhow::ensure!(
            std::path::Path::new(&git_dir).is_absolute(),
            "Git directory must be absolute"
        );
        let top = self.run(&args(&["rev-parse", "--show-toplevel"])).await?;
        anyhow::ensure!(top.code == 0, "Cannot read the worktree root");
        let worktree_root = output_line(&top.stdout, "worktree root", true)?;
        let format = self.run(&args(&["rev-parse", "--show-ref-format"])).await?;
        anyhow::ensure!(format.code == 0, "Cannot read the ref format");
        let ref_format = output_line(&format.stdout, "ref format", false)?;
        match std::fs::symlink_metadata(std::path::Path::new(&git_dir).join("HEAD")) {
            Ok(metadata) if !metadata.is_file() => {
                return Ok(invalid(
                    "Assisted commits require a regular HEAD file; symlink HEAD is not supported",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let symbolic = self
            .run(&args(&["symbolic-ref", "--quiet", "--no-recurse", "HEAD"]))
            .await?;
        anyhow::ensure!(matches!(symbolic.code, 0 | 1), "Cannot read symbolic HEAD");
        let reference = if symbolic.code == 0 {
            Some(output_line(&symbolic.stdout, "symbolic HEAD", false)?)
        } else {
            None
        };
        if let Some(reference) = &reference {
            if !reference.starts_with("refs/heads/") {
                return Ok(invalid(
                    "Assisted commits require a branch or a detached commit",
                ));
            }
            let alias = self
                .run(&args(&[
                    "symbolic-ref",
                    "--quiet",
                    "--no-recurse",
                    reference,
                ]))
                .await?;
            anyhow::ensure!(
                matches!(alias.code, 0 | 1),
                "Cannot read branch symbolic ref"
            );
            if alias.code == 0 {
                return Ok(invalid(
                    "Assisted commits do not support symbolic branch aliases",
                ));
            }
        }

        let tip = self
            .run(&args(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                "HEAD^{commit}",
            ]))
            .await?;
        anyhow::ensure!(matches!(tip.code, 0 | 1), "Cannot read HEAD commit");
        let sha = if tip.code == 0 {
            Some(output_line(&tip.stdout, "HEAD commit", false)?)
        } else {
            None
        };
        if sha.as_deref().is_some_and(|value| {
            !matches!(value.len(), 40 | 64)
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Ok(invalid("HEAD is not a full object ID"));
        }
        if reference.is_none() && sha.is_none() {
            return Ok(invalid("An unborn detached HEAD cannot be committed"));
        }
        Ok(json!({
            "kind":"ok",
            "git_dir":git_dir,
            "worktree_root":worktree_root,
            "ref_format":ref_format,
            "ref":reference,
            "sha":sha,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::Repository;
    use std::{fs, process::Command};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("desktop-assisted-head-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let fixture = Self(root);
            fixture.git(&["init", "-q", "-b", "main"]);
            fixture.git(&["config", "user.name", "Test"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
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

        fn commit(&self) -> String {
            fs::write(self.0.join("file"), "content\n").unwrap();
            self.git(&["add", "file"]);
            self.git(&["commit", "-qm", "commit"]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn safety_detects_grafts_operations_and_conflicts() {
        let fixture = Fixture::new();
        fixture.commit();
        let repository = Repository::open(&fixture.0).await.unwrap();
        assert_eq!(
            repository.assisted_commit_safety().await.unwrap()["kind"],
            "ok"
        );
        let grafts = fixture.0.join(".git/info/grafts");
        fs::write(&grafts, "0000000000000000000000000000000000000000\n").unwrap();
        let grafted = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(grafted["kind"], "invalid");
        assert!(
            grafted["message"]
                .as_str()
                .unwrap()
                .contains("grafted history")
        );
        fs::remove_file(&grafts).unwrap();
        let merge_head = fixture.0.join(".git/MERGE_HEAD");
        fs::write(&merge_head, "0000000000000000000000000000000000000000\n").unwrap();
        let merging = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(merging["kind"], "invalid");
        assert!(merging["message"].as_str().unwrap().contains("MERGE_HEAD"));
        fs::remove_file(&merge_head).unwrap();
        let blob = fixture.git(&["hash-object", "-w", "file"]);
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&fixture.0)
            .args(["update-index", "--index-info"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        write!(
            child.stdin.as_mut().unwrap(),
            "100644 {blob} 1\tfile\n100644 {blob} 2\tfile\n"
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        let conflicted = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(conflicted["kind"], "invalid");
        assert!(
            conflicted["message"]
                .as_str()
                .unwrap()
                .contains("unresolved conflicts")
        );
    }

    #[tokio::test]
    async fn reads_attached_detached_and_unborn_head() {
        let fixture = Fixture::new();
        let head = fixture.commit();
        let repository = Repository::open(&fixture.0).await.unwrap();
        let attached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(attached["kind"], "ok");
        assert_eq!(attached["ref"], "refs/heads/main");
        assert_eq!(attached["sha"], head);
        assert!(attached["git_dir"].as_str().unwrap().ends_with(".git"));
        assert_eq!(
            attached["worktree_root"],
            fixture.0.to_string_lossy().as_ref()
        );
        assert_eq!(attached["ref_format"], "files");

        fixture.git(&["switch", "-q", "--detach", "HEAD"]);
        let detached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(detached["ref"], serde_json::Value::Null);
        assert_eq!(detached["sha"], head);

        fixture.git(&["switch", "-q", "--orphan", "unborn"]);
        let unborn = repository.assisted_commit_head().await.unwrap();
        assert_eq!(unborn["kind"], "ok");
        assert_eq!(unborn["ref"], "refs/heads/unborn");
        assert_eq!(unborn["sha"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn reports_branch_aliases_and_unborn_detached_head_as_invalid() {
        let fixture = Fixture::new();
        fixture.commit();
        fixture.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
        fixture.git(&["switch", "-q", "alias"]);
        let repository = Repository::open(&fixture.0).await.unwrap();
        let aliased = repository.assisted_commit_head().await.unwrap();
        assert_eq!(aliased["kind"], "invalid");
        assert!(
            aliased["message"]
                .as_str()
                .unwrap()
                .contains("symbolic branch aliases")
        );

        fs::write(
            fixture.0.join(".git/HEAD"),
            "0000000000000000000000000000000000000000\n",
        )
        .unwrap();
        let unborn_detached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(unborn_detached["kind"], "invalid");
        assert!(
            unborn_detached["message"]
                .as_str()
                .unwrap()
                .contains("unborn detached HEAD")
        );
    }

    #[tokio::test]
    async fn rejects_symlinked_head_as_invalid() {
        let fixture = Fixture::new();
        fixture.commit();
        #[cfg(unix)]
        {
            let head = fixture.0.join(".git/HEAD");
            let original = fs::read(&head).unwrap();
            fs::remove_file(&head).unwrap();
            std::os::unix::fs::symlink("refs/heads/main", &head).unwrap();
            let repository = Repository::open(&fixture.0).await.unwrap();
            let value = repository.assisted_commit_head().await.unwrap();
            assert_eq!(value["kind"], "invalid");
            assert!(value["message"].as_str().unwrap().contains("regular HEAD"));
            fs::remove_file(&head).unwrap();
            fs::write(&head, original).unwrap();
        }
    }
}
