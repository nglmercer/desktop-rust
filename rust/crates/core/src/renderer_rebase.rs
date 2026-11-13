//! Reviewed continuation of a conflicted rebase.
//!
//! External Git writers can still race between review and Git taking locks.
use crate::{Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn renderer_rebase_review(&self, expected: &str) -> Result<String> {
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
            "GIT_SHALLOW_FILE",
        ] {
            anyhow::ensure!(
                std::env::var_os(variable).is_none(),
                "Cannot review rebase with {variable} set"
            );
        }
        anyhow::ensure!(
            (expected.len() == 40 || expected.len() == 64)
                && expected
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "A complete rebase object ID is required"
        );
        let state = self.renderer_rebase_state().await?;
        anyhow::ensure!(
            state["head"].as_str() == Some(expected),
            "Rebase changed; refresh before continuing"
        );
        // The merge backend's complete progress state is captured below; do not
        // claim review support for apply-backend state yet.
        anyhow::ensure!(
            state["progress"].is_object(),
            "Only merge-backend rebase continuation is supported"
        );
        anyhow::ensure!(
            self.operation_state()
                .await?
                .is_some_and(|op| op.kind == "rebase"),
            "The active Git operation is not a rebase"
        );
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(
            unmerged.code == 0 && unmerged.stdout.is_empty(),
            "Resolve and stage conflicts before continuing"
        );
        let capture = self.checkout_capture(0).await?;
        anyhow::ensure!(
            self.renderer_rebase_state().await? == state,
            "Rebase changed during review"
        );
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            expected.as_bytes(),
            &serde_json::to_vec(&state)?,
            &serde_json::to_vec(&capture)?,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_rebase_continue_preview(&self, expected: &str) -> Result<Value> {
        Ok(json!({"basis":self.renderer_rebase_review(expected).await?,"head":expected}))
    }

    pub(crate) async fn renderer_rebase_continue(
        &self,
        expected: &str,
        basis: &str,
        empty: bool,
        no_verify: bool,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_rebase_review(expected).await? == basis,
            "Rebase context changed; refresh before continuing"
        );
        if empty {
            for diff in [
                args(&["diff", "--quiet", "--"]),
                args(&["diff", "--cached", "--quiet", "--"]),
            ] {
                anyhow::ensure!(
                    self.run(&diff).await?.code == 0,
                    "Rebase no longer has an empty tracked state"
                );
            }
        }
        let mut command = args(&["rebase", if empty { "--skip" } else { "--continue" }]);
        if no_verify {
            command.extend(args(&["--no-verify"]));
        }
        let output = process::run_bound_noninteractive(&self.path, &command).await?;
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(unmerged.code == 0, "Cannot inspect rebase outcome");
        let state = self.renderer_rebase_state().await?;
        let outcome = if output.code == 0 {
            "complete"
        } else if !unmerged.stdout.is_empty() && !state.is_null() {
            "conflict"
        } else {
            "failed"
        };
        Ok(
            json!({"outcome":outcome,"code":output.code,"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    struct Fixture(PathBuf);
    impl Fixture {
        fn run(&self, args: &[&str]) -> std::process::Output {
            Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap()
        }
        fn git(&self, args: &[&str]) -> String {
            let out = self.run(args);
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().into()
        }
        fn new() -> (Self, String) {
            let root =
                std::env::temp_dir().join(format!("desktop-rebase-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.git(&["init", "-q", "-b", "main"]);
            f.git(&["config", "user.name", "Test"]);
            f.git(&["config", "user.email", "test@example.invalid"]);
            f.git(&["config", "commit.gpgsign", "false"]);
            fs::write(f.0.join("file"), "base\n").unwrap();
            f.git(&["add", "file"]);
            f.git(&["commit", "-qm", "base"]);
            f.git(&["switch", "-qc", "source"]);
            fs::write(f.0.join("file"), "source\n").unwrap();
            f.git(&["commit", "-qam", "source"]);
            let expected = f.git(&["rev-parse", "HEAD"]);
            f.git(&["switch", "-q", "main"]);
            fs::write(f.0.join("file"), "main\n").unwrap();
            f.git(&["commit", "-qam", "main"]);
            f.git(&["switch", "-q", "source"]);
            assert!(!f.run(&["rebase", "main"]).status.success());
            (f, expected)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn rejects_unresolved_or_stale_review_and_continues() {
        let (f, expected) = Fixture::new();
        let repo = Repository::open(&f.0).await.unwrap();
        assert!(
            repo.renderer_rebase_continue_preview(&expected)
                .await
                .is_err()
        );
        fs::write(f.0.join("file"), "resolved\n").unwrap();
        f.git(&["add", "file"]);
        let review = repo
            .renderer_rebase_continue_preview(&expected)
            .await
            .unwrap();
        let todo = f.0.join(".git/rebase-merge/git-rebase-todo");
        let original = fs::read(&todo).unwrap();
        fs::write(&todo, b"# changed after review\n").unwrap();
        assert!(
            repo.renderer_rebase_continue(
                &expected,
                review["basis"].as_str().unwrap(),
                false,
                false
            )
            .await
            .is_err()
        );
        fs::write(todo, original).unwrap();
        fs::write(f.0.join("file"), "changed\n").unwrap();
        f.git(&["add", "file"]);
        assert!(
            repo.renderer_rebase_continue(
                &expected,
                review["basis"].as_str().unwrap(),
                false,
                false
            )
            .await
            .is_err()
        );
        let review = repo
            .renderer_rebase_continue_preview(&expected)
            .await
            .unwrap();
        assert!(
            repo.renderer_rebase_continue(
                &expected,
                review["basis"].as_str().unwrap(),
                true,
                false
            )
            .await
            .is_err()
        );
        let result = repo
            .renderer_rebase_continue(&expected, review["basis"].as_str().unwrap(), false, false)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(f.git(&["show", "HEAD:file"]), "changed");
        assert!(repo.renderer_rebase_state().await.unwrap().is_null());
    }

    #[tokio::test]
    async fn skips_empty_resolution() {
        let (f, expected) = Fixture::new();
        let repo = Repository::open(&f.0).await.unwrap();
        fs::write(f.0.join("file"), "main\n").unwrap();
        f.git(&["add", "file"]);
        let review = repo
            .renderer_rebase_continue_preview(&expected)
            .await
            .unwrap();
        let result = repo
            .renderer_rebase_continue(&expected, review["basis"].as_str().unwrap(), true, false)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(f.git(&["show", "HEAD:file"]), "main");
        assert!(repo.renderer_rebase_state().await.unwrap().is_null());
    }

    #[tokio::test]
    async fn reports_conflict_in_a_later_commit() {
        let (f, _) = Fixture::new();
        f.git(&["rebase", "--abort"]);
        fs::write(f.0.join("file"), "second\n").unwrap();
        f.git(&["commit", "-qam", "second"]);
        assert!(!f.run(&["rebase", "main"]).status.success());
        let expected = f.git(&["rev-parse", "REBASE_HEAD"]);
        fs::write(f.0.join("file"), "resolved first\n").unwrap();
        f.git(&["add", "file"]);
        let repo = Repository::open(&f.0).await.unwrap();
        let review = repo
            .renderer_rebase_continue_preview(&expected)
            .await
            .unwrap();
        let result = repo
            .renderer_rebase_continue(&expected, review["basis"].as_str().unwrap(), false, false)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "conflict");
        assert!(!f.git(&["ls-files", "--unmerged"]).is_empty());
        assert_ne!(
            repo.renderer_rebase_state().await.unwrap()["head"],
            expected
        );
    }
}
