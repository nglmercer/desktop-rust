//! Reviewed continuation of a conflicted cherry-pick.
//!
//! This coordinates Desktop operations, but cannot reserve external Git
//! writers between reviewing the repository and Git taking its own locks.
use crate::{Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn renderer_cherry_pick_review(&self, expected: &str) -> Result<String> {
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
                "Cannot review cherry-pick with {variable} set"
            );
        }
        anyhow::ensure!(
            (expected.len() == 40 || expected.len() == 64)
                && expected
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "A complete cherry-pick object ID is required"
        );
        let state = self.renderer_cherry_pick_state().await?;
        anyhow::ensure!(
            self.operation_state()
                .await?
                .is_some_and(|operation| operation.kind == "cherry_pick"),
            "The active Git operation is not a cherry-pick"
        );
        anyhow::ensure!(
            state["head"].as_str() == Some(expected),
            "Cherry-pick changed; refresh before continuing"
        );
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(
            unmerged.code == 0 && unmerged.stdout.is_empty(),
            "Resolve and stage conflicts before continuing"
        );
        let capture = self.checkout_capture(0).await?;
        anyhow::ensure!(
            self.renderer_cherry_pick_state().await? == state,
            "Cherry-pick changed during review"
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

    pub(crate) async fn renderer_cherry_pick_continue_preview(
        &self,
        expected: &str,
    ) -> Result<Value> {
        Ok(json!({"basis": self.renderer_cherry_pick_review(expected).await?, "head":expected}))
    }

    pub(crate) async fn renderer_cherry_pick_continue(
        &self,
        expected: &str,
        basis: &str,
        empty: bool,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_cherry_pick_review(expected).await? == basis,
            "Cherry-pick context changed; refresh before continuing"
        );
        if empty {
            for diff in [
                args(&["diff", "--quiet", "--"]),
                args(&["diff", "--cached", "--quiet", "--"]),
            ] {
                anyhow::ensure!(
                    self.run(&diff).await?.code == 0,
                    "Cherry-pick no longer has an empty tracked state"
                );
            }
        }
        // Match the renderer's existing empty-pick behavior; Git continues to
        // own the sequencer, hooks and author/message semantics.
        let command = if empty {
            args(&["-c", "core.editor=true", "commit", "--allow-empty"])
        } else {
            args(&["-c", "core.editor=true", "cherry-pick", "--continue"])
        };
        let output = process::run_bound_cherry_pick(&self.path, &command).await?;
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(unmerged.code == 0, "Cannot inspect cherry-pick outcome");
        let state = self.renderer_cherry_pick_state().await?;
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
        fn git(&self, args: &[&str]) -> std::process::Output {
            Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap()
        }
        fn ok(&self, args: &[&str]) -> String {
            let out = self.git(args);
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        }
        fn new() -> (Self, String) {
            let root = std::env::temp_dir().join(format!("desktop-pick-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.ok(&["init", "-q", "-b", "main"]);
            f.ok(&["config", "user.name", "Test"]);
            f.ok(&["config", "user.email", "test@example.invalid"]);
            f.ok(&["config", "commit.gpgsign", "false"]);
            fs::write(f.0.join("file"), "base\n").unwrap();
            f.ok(&["add", "file"]);
            f.ok(&["commit", "-qm", "base"]);
            f.ok(&["switch", "-qc", "source"]);
            fs::write(f.0.join("file"), "source\n").unwrap();
            f.ok(&["commit", "-qam", "source"]);
            let target = f.ok(&["rev-parse", "HEAD"]);
            f.ok(&["switch", "-q", "main"]);
            fs::write(f.0.join("file"), "main\n").unwrap();
            f.ok(&["commit", "-qam", "main"]);
            assert!(!f.git(&["cherry-pick", &target]).status.success());
            (f, target)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn continuation_rejects_conflicts_and_stale_index_then_commits_resolution() {
        let (f, target) = Fixture::new();
        let repo = Repository::open(&f.0).await.unwrap();
        assert!(
            repo.renderer_cherry_pick_continue_preview(&target)
                .await
                .is_err()
        );
        fs::write(f.0.join("file"), "resolution\n").unwrap();
        f.ok(&["add", "file"]);
        fs::create_dir(f.0.join(".git/rebase-merge")).unwrap();
        assert!(
            repo.renderer_cherry_pick_continue_preview(&target)
                .await
                .is_err()
        );
        fs::remove_dir(f.0.join(".git/rebase-merge")).unwrap();
        let preview = repo
            .renderer_cherry_pick_continue_preview(&target)
            .await
            .unwrap();
        fs::write(f.0.join("file"), "different\n").unwrap();
        f.ok(&["add", "file"]);
        assert!(
            repo.renderer_cherry_pick_continue(&target, preview["basis"].as_str().unwrap(), false)
                .await
                .is_err()
        );
        fs::write(f.0.join("file"), "resolution\n").unwrap();
        f.ok(&["add", "file"]);
        let preview = repo
            .renderer_cherry_pick_continue_preview(&target)
            .await
            .unwrap();
        assert!(
            repo.renderer_cherry_pick_continue(&target, preview["basis"].as_str().unwrap(), true)
                .await
                .is_err()
        );
        let result = repo
            .renderer_cherry_pick_continue(&target, preview["basis"].as_str().unwrap(), false)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(f.ok(&["show", "HEAD:file"]), "resolution");
        assert!(repo.renderer_cherry_pick_state().await.unwrap().is_null());
    }

    #[tokio::test]
    async fn continuation_reports_conflict_in_the_next_commit() {
        let (f, target) = Fixture::new();
        f.ok(&["cherry-pick", "--abort"]);
        f.ok(&["switch", "-q", "source"]);
        fs::write(f.0.join("file"), "second\n").unwrap();
        f.ok(&["commit", "-qam", "second"]);
        let second = f.ok(&["rev-parse", "HEAD"]);
        f.ok(&["switch", "-q", "main"]);
        assert!(!f.git(&["cherry-pick", &target, &second]).status.success());
        fs::write(f.0.join("file"), "resolution\n").unwrap();
        f.ok(&["add", "file"]);
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo
            .renderer_cherry_pick_continue_preview(&target)
            .await
            .unwrap();
        let result = repo
            .renderer_cherry_pick_continue(&target, preview["basis"].as_str().unwrap(), false)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "conflict");
        assert_eq!(
            repo.renderer_cherry_pick_state().await.unwrap()["head"],
            second
        );
        assert!(!f.ok(&["ls-files", "--unmerged"]).is_empty());
    }

    #[tokio::test]
    async fn empty_resolution_creates_an_empty_commit() {
        if std::env::var_os("DESKTOP_PICK_EDITOR_TEST_CHILD").is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "renderer_cherry_pick::tests::empty_resolution_creates_an_empty_commit",
                    "--nocapture",
                ])
                .env("DESKTOP_PICK_EDITOR_TEST_CHILD", "1")
                .env("GIT_EDITOR", "false")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let (f, target) = Fixture::new();
        let original = f.ok(&["rev-parse", "HEAD"]);
        f.ok(&["checkout", "--ours", "--", "file"]);
        f.ok(&["add", "file"]);
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo
            .renderer_cherry_pick_continue_preview(&target)
            .await
            .unwrap();
        let result = repo
            .renderer_cherry_pick_continue(&target, preview["basis"].as_str().unwrap(), true)
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_ne!(f.ok(&["rev-parse", "HEAD"]), original);
        assert_eq!(
            f.ok(&["rev-parse", "HEAD^{tree}"]),
            f.ok(&["rev-parse", &format!("{original}^{{tree}}")])
        );
        assert!(repo.renderer_cherry_pick_state().await.unwrap().is_null());
    }
}
