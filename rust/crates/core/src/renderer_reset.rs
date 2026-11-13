//! Reviewed mixed reset for undoing a commit or selecting a history point.
//! External Git writers may race review and Git taking its own locks.
use crate::{RendererMixedResetRequest, Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn renderer_mixed_reset_review(
        &self,
        request: &RendererMixedResetRequest,
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
                "Cannot review reset with {variable} set"
            );
        }
        let oid = |value: &str| {
            matches!(value.len(), 40 | 64)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        anyhow::ensure!(
            oid(&request.head)
                && oid(&request.target)
                && request.head.len() == request.target.len(),
            "Complete commit IDs are required"
        );
        anyhow::ensure!(
            request.branch.as_deref().is_none_or(
                |name| name.starts_with("refs/heads/") && !name.contains(['\0', '\n', '\r'])
            ),
            "Invalid current branch"
        );
        anyhow::ensure!(
            self.resolve_commit(&request.target).await? == request.target,
            "Selected target no longer resolves to the same commit"
        );
        if request.undo {
            let parents = self
                .run(&args(&[
                    "rev-list",
                    "--parents",
                    "--max-count=1",
                    &request.head,
                    "--",
                ]))
                .await?;
            let text = std::str::from_utf8(&parents.stdout)?;
            let mut fields = text.split_whitespace();
            anyhow::ensure!(
                parents.code == 0
                    && fields.next() == Some(request.head.as_str())
                    && fields.next() == Some(request.target.as_str()),
                "The selected commit or its first parent changed"
            );
        }
        anyhow::ensure!(
            self.operation_state().await?.is_none(),
            "Finish or abort the current Git operation first"
        );
        match std::fs::symlink_metadata(self.git_directory().await?.join("SQUASH_MSG")) {
            Ok(_) => anyhow::bail!("Finish or abort the pending squash before resetting"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let capture = self.checkout_capture(0).await?;
        let branch = request
            .branch
            .as_deref()
            .and_then(|value| value.strip_prefix("refs/heads/"));
        anyhow::ensure!(
            capture["snapshot"]["head"].as_str() == Some(&request.head)
                && capture["snapshot"]["branch"].as_str() == branch
                && capture["snapshot"]["operation"].is_null(),
            "Current branch changed; refresh before resetting"
        );
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &serde_json::to_vec(&capture)?,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_mixed_reset_preview(
        &self,
        request: &RendererMixedResetRequest,
    ) -> Result<Value> {
        Ok(json!({"request":request,"basis":self.renderer_mixed_reset_review(request).await?}))
    }

    pub(crate) async fn renderer_mixed_reset(
        &self,
        request: &RendererMixedResetRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_mixed_reset_review(request).await? == basis,
            "Reset context changed; refresh before continuing"
        );
        let output = process::run_bound_checkout(
            &self.path,
            &args(&["reset", "--mixed", &request.target, "--"]),
        )
        .await?;
        if output.code == 0 {
            let head = self
                .run(&args(&[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    "HEAD^{commit}",
                ]))
                .await?;
            let branch = self
                .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
                .await?;
            let actual = if branch.code == 0 {
                Some(std::str::from_utf8(&branch.stdout)?.trim_end_matches('\n'))
            } else if branch.code == 1 {
                None
            } else {
                anyhow::bail!("Reset outcome is uncertain; inspect HEAD and index")
            };
            anyhow::ensure!(
                head.code == 0
                    && std::str::from_utf8(&head.stdout)?.trim() == request.target
                    && actual == request.branch.as_deref(),
                "Reset outcome is uncertain; inspect HEAD and index"
            );
        }
        Ok(
            json!({"code":output.code,"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),"target":request.target}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};
    struct Fixture(PathBuf);
    impl Fixture {
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().into()
        }
        fn new() -> (Self, RendererMixedResetRequest) {
            let root = std::env::temp_dir().join(format!("desktop-reset-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.git(&["init", "-q", "-b", "main"]);
            f.git(&["config", "user.name", "Test"]);
            f.git(&["config", "user.email", "test@example.invalid"]);
            f.git(&["config", "commit.gpgsign", "false"]);
            fs::write(f.0.join("file"), "base\n").unwrap();
            f.git(&["add", "file"]);
            f.git(&["commit", "-qm", "base"]);
            let target = f.git(&["rev-parse", "HEAD"]);
            fs::write(f.0.join("file"), "commit\n").unwrap();
            f.git(&["commit", "-qam", "commit"]);
            let head = f.git(&["rev-parse", "HEAD"]);
            (
                f,
                RendererMixedResetRequest {
                    target,
                    head,
                    branch: Some("refs/heads/main".into()),
                    undo: false,
                },
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn reset_preserves_working_bytes_and_moves_head_and_index() {
        let (f, request) = Fixture::new();
        fs::write(f.0.join("file"), "staged\n").unwrap();
        f.git(&["add", "file"]);
        fs::write(f.0.join("file"), "worktree\n").unwrap();
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_mixed_reset_preview(&request).await.unwrap();
        let result = repo
            .renderer_mixed_reset(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), request.target);
        assert_eq!(f.git(&["show", ":file"]), "base");
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "worktree\n");
        assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    }

    #[tokio::test]
    async fn reset_rejects_changed_content_index_branch_and_pending_operation() {
        let (f, request) = Fixture::new();
        let repo = Repository::open(&f.0).await.unwrap();
        fs::write(f.0.join("file"), "local-a\n").unwrap();
        let preview = repo.renderer_mixed_reset_preview(&request).await.unwrap();
        fs::write(f.0.join("file"), "local-b\n").unwrap();
        assert!(
            repo.renderer_mixed_reset(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "local-b\n");
        let preview = repo.renderer_mixed_reset_preview(&request).await.unwrap();
        f.git(&["add", "file"]);
        assert!(
            repo.renderer_mixed_reset(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["reset", "--hard", "-q", "HEAD"]);
        let preview = repo.renderer_mixed_reset_preview(&request).await.unwrap();
        f.git(&["switch", "-qc", "other"]);
        assert!(
            repo.renderer_mixed_reset(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["switch", "-q", "main"]);
        fs::write(f.0.join(".git/SQUASH_MSG"), "pending\n").unwrap();
        assert!(repo.renderer_mixed_reset_preview(&request).await.is_err());
        assert_eq!(f.git(&["rev-parse", "HEAD"]), request.head);
    }

    #[tokio::test]
    async fn undo_requires_exact_first_parent_and_supports_detached_head() {
        let (f, mut request) = Fixture::new();
        request.undo = true;
        let repo = Repository::open(&f.0).await.unwrap();
        let mut invalid = request.clone();
        invalid.target = request.head.clone();
        assert!(repo.renderer_mixed_reset_preview(&invalid).await.is_err());
        f.git(&["switch", "--detach", "HEAD"]);
        request.branch = None;
        let preview = repo.renderer_mixed_reset_preview(&request).await.unwrap();
        let result = repo
            .renderer_mixed_reset(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(f.git(&["rev-parse", "HEAD"]), request.target);
        assert!(
            !Command::new("git")
                .arg("-C")
                .arg(&f.0)
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "commit\n");
    }
}
