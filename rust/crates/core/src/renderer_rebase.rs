//! Reviewed continuation of a conflicted rebase.
//!
//! External Git writers can still race between review and Git taking locks.
use crate::{GitPath, RendererRebaseStartRequest, Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn renderer_rebase_start_review(
        &self,
        request: &RendererRebaseStartRequest,
    ) -> Result<String> {
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
        let oid = |value: &str| {
            matches!(value.len(), 40 | 64)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        anyhow::ensure!(
            oid(&request.current_head)
                && oid(&request.base_head)
                && oid(&request.target_head)
                && request.base_head.len() == request.current_head.len()
                && request.target_head.len() == request.current_head.len(),
            "Complete branch object IDs are required"
        );
        anyhow::ensure!(
            request.base_ref.starts_with("refs/heads/")
                || request.base_ref.starts_with("refs/remotes/"),
            "Invalid base branch ref"
        );
        let short = request
            .target_ref
            .strip_prefix("refs/heads/")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("A local target branch is required"))?;
        anyhow::ensure!(
            request.current_branch.as_deref().is_none_or(
                |name| name.starts_with("refs/heads/") && !name.contains(['\0', '\n', '\r'])
            ),
            "Invalid current branch"
        );
        for reference in [&request.base_ref, &request.target_ref] {
            anyhow::ensure!(
                self.run(&args(&["check-ref-format", reference]))
                    .await?
                    .code
                    == 0,
                "Invalid branch reference"
            );
        }
        anyhow::ensure!(
            self.resolve_commit(&request.base_ref).await? == request.base_head
                && self.resolve_commit(&request.target_ref).await? == request.target_head,
            "Selected branches changed; refresh before rebasing"
        );
        // A canonical refs/heads/ argument makes Git detach HEAD during rebase.
        // Resolve the option-terminated short name and refuse ambiguous names.
        let symbolic = self
            .run(&args(&[
                "-c",
                "core.warnAmbiguousRefs=true",
                "rev-parse",
                "--symbolic-full-name",
                "--verify",
                "--end-of-options",
                short,
            ]))
            .await?;
        anyhow::ensure!(
            symbolic.code == 0
                && symbolic.stderr.is_empty()
                && std::str::from_utf8(&symbolic.stdout)?.trim_end_matches('\n')
                    == request.target_ref,
            "Target branch name is ambiguous"
        );
        anyhow::ensure!(
            self.operation_state().await?.is_none(),
            "Finish or abort the current Git operation first"
        );
        match std::fs::symlink_metadata(self.git_directory().await?.join("SQUASH_MSG")) {
            Ok(_) => anyhow::bail!("Finish or abort the pending squash before rebasing"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let capture = self.checkout_capture(0).await?;
        let expected_branch = request
            .current_branch
            .as_deref()
            .and_then(|value| value.strip_prefix("refs/heads/"));
        anyhow::ensure!(
            capture["snapshot"]["head"].as_str() == Some(&request.current_head)
                && capture["snapshot"]["branch"].as_str() == expected_branch
                && capture["snapshot"]["operation"].is_null(),
            "Current branch changed; refresh before rebasing"
        );
        let worktrees = self.worktrees().await?;
        let entries = worktrees
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Cannot inspect linked worktrees"))?;
        for entry in entries {
            if entry["branch"].as_str() == Some(&request.target_ref) {
                let path = match serde_json::from_value::<GitPath>(entry["path"].clone())? {
                    GitPath::Bytes { bytes } => {
                        std::path::PathBuf::from(crate::os_from_bytes(&bytes)?)
                    }
                    GitPath::Text(path) => std::path::PathBuf::from(path),
                };
                anyhow::ensure!(
                    path.canonicalize().ok().as_ref() == Some(&self.path),
                    "Target branch is checked out in another worktree"
                );
            }
        }
        anyhow::ensure!(
            self.resolve_commit(&request.base_ref).await? == request.base_head
                && self.resolve_commit(&request.target_ref).await? == request.target_head
                && self.operation_state().await?.is_none(),
            "Repository changed during review"
        );
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &serde_json::to_vec(&capture)?,
            &serde_json::to_vec(&worktrees)?,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_rebase_start_preview(
        &self,
        request: &RendererRebaseStartRequest,
    ) -> Result<Value> {
        Ok(json!({"request":request,"basis":self.renderer_rebase_start_review(request).await?}))
    }

    pub(crate) async fn renderer_rebase_start(
        &self,
        request: &RendererRebaseStartRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_rebase_start_review(request).await? == basis,
            "Rebase context changed; refresh before starting"
        );
        let short = request
            .target_ref
            .strip_prefix("refs/heads/")
            .ok_or_else(|| anyhow::anyhow!("Invalid target branch"))?;
        let command = args(&[
            "-c",
            "rebase.backend=merge",
            "rebase",
            "--",
            &request.base_head,
            short,
        ]);
        let output = process::run_bound_noninteractive(&self.path, &command).await?;
        if output.code == 0 {
            let branch = self
                .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
                .await?;
            anyhow::ensure!(
                branch.code == 0
                    && std::str::from_utf8(&branch.stdout)?.trim_end_matches('\n')
                        == request.target_ref,
                "Rebase outcome is uncertain; inspect current branch and history"
            );
        }
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(unmerged.code == 0, "Cannot inspect rebase outcome");
        let state = self.renderer_rebase_state().await?;
        let text = String::from_utf8_lossy(&output.stdout);
        let outcome = if output.code == 0
            && text.trim().starts_with("Current branch ")
            && text.trim().ends_with(" is up to date.")
        {
            "up_to_date"
        } else if output.code == 0 {
            "complete"
        } else if !unmerged.stdout.is_empty() && !state.is_null() {
            "conflict"
        } else {
            "failed"
        };
        Ok(
            json!({"outcome":outcome,"code":output.code,"stdout":text,"stderr":String::from_utf8_lossy(&output.stderr)}),
        )
    }

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
            state["backend"] == "merge" && state["progress"].is_object(),
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

    fn start_request(f: &Fixture, target: &str) -> RendererRebaseStartRequest {
        RendererRebaseStartRequest {
            base_ref: "refs/heads/main".into(),
            base_head: f.git(&["rev-parse", "refs/heads/main"]),
            target_ref: format!("refs/heads/{target}"),
            target_head: f.git(&["rev-parse", &format!("refs/heads/{target}")]),
            current_head: f.git(&["rev-parse", "HEAD"]),
            current_branch: {
                let out = f.run(&["symbolic-ref", "--quiet", "HEAD"]);
                if out.status.success() {
                    Some(String::from_utf8(out.stdout).unwrap().trim().into())
                } else {
                    None
                }
            },
        }
    }

    #[tokio::test]
    async fn start_handles_option_shaped_and_unicode_branches_from_detached_head() {
        for name in ["-CaseSensitive", "Tópico/Case"] {
            let (f, _) = Fixture::new();
            f.git(&["rebase", "--abort"]);
            let base = f.git(&["rev-parse", "main^"]);
            f.git(&["update-ref", &format!("refs/heads/{name}"), &base]);
            f.git(&["switch", "--", name]);
            fs::write(f.0.join("extra"), "feature\n").unwrap();
            f.git(&["add", "extra"]);
            f.git(&["commit", "-qm", "feature"]);
            f.git(&["switch", "--detach", "main"]);
            let request = start_request(&f, name);
            let repo = Repository::open(&f.0).await.unwrap();
            let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
            let result = repo
                .renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(result["outcome"], "complete");
            assert_eq!(
                f.git(&["symbolic-ref", "HEAD"]),
                format!("refs/heads/{name}")
            );
            assert_eq!(f.git(&["show", "HEAD:extra"]), "feature");
        }
    }

    #[tokio::test]
    async fn start_rejects_stale_refs_and_worktree_and_reports_conflict() {
        let (f, _) = Fixture::new();
        f.git(&["rebase", "--abort"]);
        let request = start_request(&f, "source");
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
        f.git(&["tag", "source", &request.target_head]);
        assert!(
            repo.renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["tag", "-d", "source"]);
        fs::write(f.0.join("file"), "changed locally\n").unwrap();
        assert!(
            repo.renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["reset", "--hard", "-q", "HEAD"]);
        let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
        f.git(&["update-ref", "refs/heads/main", &request.current_head]);
        assert!(
            repo.renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["update-ref", "refs/heads/main", &request.base_head]);
        let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
        f.git(&["update-ref", "refs/heads/source", &request.base_head]);
        assert!(
            repo.renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["update-ref", "refs/heads/source", &request.target_head]);
        let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
        let result = repo
            .renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "conflict");
        assert!(!f.git(&["ls-files", "--unmerged"]).is_empty());
        assert_eq!(
            repo.renderer_rebase_state().await.unwrap()["head"],
            request.target_head
        );
    }

    #[tokio::test]
    async fn start_rejects_ambiguous_and_nonlocal_refs() {
        let (f, _) = Fixture::new();
        f.git(&["rebase", "--abort"]);
        let request = start_request(&f, "source");
        let repo = Repository::open(&f.0).await.unwrap();
        f.git(&["tag", "source", &request.target_head]);
        assert!(repo.renderer_rebase_start_preview(&request).await.is_err());
        f.git(&["config", "core.warnAmbiguousRefs", "false"]);
        assert!(repo.renderer_rebase_start_preview(&request).await.is_err());
        f.git(&["tag", "-d", "source"]);
        f.git(&["update-ref", "refs/remotes/source", &request.target_head]);
        assert!(repo.renderer_rebase_start_preview(&request).await.is_err());
        let mut invalid = request.clone();
        invalid.target_ref = "--help".into();
        assert!(repo.renderer_rebase_start_preview(&invalid).await.is_err());
    }

    #[tokio::test]
    async fn start_rejects_target_checked_out_in_another_worktree() {
        let (f, _) = Fixture::new();
        f.git(&["rebase", "--abort"]);
        let linked =
            std::env::temp_dir().join(format!("desktop-rebase-linked-{}", uuid::Uuid::new_v4()));
        let mut request = start_request(&f, "main");
        request.base_ref = "refs/heads/source".into();
        request.base_head = request.current_head.clone();
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_rebase_start_preview(&request).await.unwrap();
        f.git(&["worktree", "add", "-q", linked.to_str().unwrap(), "main"]);
        assert!(
            repo.renderer_rebase_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        assert!(repo.renderer_rebase_start_preview(&request).await.is_err());
        f.git(&["worktree", "remove", linked.to_str().unwrap()]);
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
