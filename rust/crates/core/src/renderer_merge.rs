//! Reviewed merge start. External Git writers can race review and Git locks.
use crate::{RendererMergeStartRequest, Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn renderer_merge_start_review(
        &self,
        request: &RendererMergeStartRequest,
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
                "Cannot review merge with {variable} set"
            );
        }
        let oid = |value: &str| {
            matches!(value.len(), 40 | 64)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        anyhow::ensure!(
            oid(&request.source_head)
                && oid(&request.current_head)
                && request.source_head.len() == request.current_head.len(),
            "Complete branch object IDs are required"
        );
        let short = request
            .source_ref
            .strip_prefix("refs/heads/")
            .or_else(|| request.source_ref.strip_prefix("refs/remotes/"))
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Invalid source branch"))?;
        anyhow::ensure!(
            request.current_ref.starts_with("refs/heads/")
                && !request.current_ref.contains(['\0', '\n', '\r']),
            "Invalid current branch"
        );
        for reference in [&request.source_ref, &request.current_ref] {
            anyhow::ensure!(
                self.run(&args(&["check-ref-format", reference]))
                    .await?
                    .code
                    == 0,
                "Invalid branch reference"
            );
        }
        anyhow::ensure!(
            self.resolve_commit(&request.source_ref).await? == request.source_head,
            "Source branch changed; refresh before merging"
        );
        // Git uses the input spelling for its merge commit message. A full
        // ref or object ID changes that message, so use a verified short name.
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
                    == request.source_ref,
            "Source branch name is ambiguous"
        );
        anyhow::ensure!(
            self.operation_state().await?.is_none(),
            "Finish or abort the current Git operation first"
        );
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(
            unmerged.code == 0 && unmerged.stdout.is_empty(),
            "Resolve existing index conflicts before merging"
        );
        if request.squash {
            let staged = self
                .run(&args(&["diff", "--cached", "--quiet", "--"]))
                .await?;
            anyhow::ensure!(
                staged.code == 0,
                "Unstage existing changes before a squash merge"
            );
        }
        match std::fs::symlink_metadata(self.git_directory().await?.join("SQUASH_MSG")) {
            Ok(_) => anyhow::bail!("Finish or abort the pending squash before merging"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let capture = self.checkout_capture(0).await?;
        anyhow::ensure!(
            capture["snapshot"]["head"].as_str() == Some(&request.current_head)
                && capture["snapshot"]["branch"].as_str()
                    == request.current_ref.strip_prefix("refs/heads/")
                && capture["snapshot"]["operation"].is_null(),
            "Current branch changed; refresh before merging"
        );
        anyhow::ensure!(
            self.resolve_commit(&request.source_ref).await? == request.source_head
                && self.operation_state().await?.is_none(),
            "Repository changed during review"
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

    pub(crate) async fn renderer_merge_start_preview(
        &self,
        request: &RendererMergeStartRequest,
    ) -> Result<Value> {
        Ok(json!({"request":request,"basis":self.renderer_merge_start_review(request).await?}))
    }

    pub(crate) async fn renderer_merge_start(
        &self,
        request: &RendererMergeStartRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_merge_start_review(request).await? == basis,
            "Merge context changed; refresh before starting"
        );
        let short = request
            .source_ref
            .strip_prefix("refs/heads/")
            .or_else(|| request.source_ref.strip_prefix("refs/remotes/"))
            .ok_or_else(|| anyhow::anyhow!("Invalid source branch"))?;
        let mut command = args(&["merge"]);
        if request.squash {
            command.extend(args(&["--squash"]));
        }
        if request.no_verify {
            command.extend(args(&["--no-verify"]));
        }
        command.extend(args(&["--", short]));
        let output = process::run_bound_noninteractive(&self.path, &command).await?;
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(unmerged.code == 0, "Cannot inspect merge outcome");
        let marker = self.git_directory().await?.join(if request.squash {
            "SQUASH_MSG"
        } else {
            "MERGE_HEAD"
        });
        let merge_marker = match std::fs::symlink_metadata(marker) {
            Ok(meta) => meta.is_file() && !meta.file_type().is_symlink(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let mut outcome = if output.code == 0 {
            if output.stdout == b"Already up to date.\n" {
                "up_to_date"
            } else {
                "complete"
            }
        } else if !unmerged.stdout.is_empty() && merge_marker {
            "conflict"
        } else {
            "failed"
        };
        let mut code = output.code;
        let mut stdout = output.stdout;
        let mut stderr = output.stderr;
        if code == 0 && request.squash {
            let commit =
                process::run_bound_noninteractive(&self.path, &args(&["commit", "--no-edit"]))
                    .await?;
            code = commit.code;
            stdout.extend(commit.stdout);
            stderr.extend(commit.stderr);
            if code != 0 {
                outcome = "failed";
            }
        }
        if code == 0 {
            let current = self
                .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
                .await?;
            anyhow::ensure!(
                current.code == 0
                    && std::str::from_utf8(&current.stdout)?.trim_end_matches('\n')
                        == request.current_ref,
                "Merge outcome is uncertain; inspect branch and history"
            );
        }
        Ok(
            json!({"outcome":outcome,"code":code,"stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr)}),
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
        fn new(source: &str, conflict: bool) -> (Self, RendererMergeStartRequest) {
            let root = std::env::temp_dir().join(format!("desktop-merge-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.git(&["init", "-q", "-b", "main"]);
            f.git(&["config", "user.name", "Test"]);
            f.git(&["config", "user.email", "test@example.invalid"]);
            f.git(&["config", "commit.gpgsign", "false"]);
            fs::write(f.0.join("file"), "base\n").unwrap();
            f.git(&["add", "file"]);
            f.git(&["commit", "-qm", "base"]);
            let base = f.git(&["rev-parse", "HEAD"]);
            f.git(&["update-ref", &format!("refs/heads/{source}"), &base]);
            f.git(&["switch", "--", source]);
            fs::write(
                f.0.join(if conflict { "file" } else { "source" }),
                "source\n",
            )
            .unwrap();
            f.git(&["add", "--all"]);
            f.git(&["commit", "-qm", "source"]);
            let source_head = f.git(&["rev-parse", "HEAD"]);
            f.git(&["switch", "-q", "main"]);
            fs::write(f.0.join(if conflict { "file" } else { "main" }), "main\n").unwrap();
            f.git(&["add", "--all"]);
            f.git(&["commit", "-qm", "main"]);
            let request = RendererMergeStartRequest {
                source_ref: format!("refs/heads/{source}"),
                source_head,
                current_ref: "refs/heads/main".into(),
                current_head: f.git(&["rev-parse", "HEAD"]),
                squash: false,
                no_verify: false,
            };
            (f, request)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn merges_the_reviewed_branch_with_the_original_message() {
        for name in ["feature", "-CaseSensitive", "Tópico/Case"] {
            let (f, request) = Fixture::new(name, false);
            let repo = Repository::open(&f.0).await.unwrap();
            let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
            let result = repo
                .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(result["outcome"], "complete");
            assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
            assert_eq!(
                f.git(&["show", "-s", "--format=%s", "HEAD"]),
                format!("Merge branch '{name}'")
            );
            assert_eq!(f.git(&["show", "HEAD:source"]), "source");
        }
    }

    #[tokio::test]
    async fn fast_forward_and_already_up_to_date_keep_result_semantics() {
        let (f, mut request) = Fixture::new("feature", false);
        f.git(&["reset", "--hard", "-q", "HEAD^"]);
        request.current_head = f.git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        let result = repo
            .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(f.git(&["rev-parse", "HEAD"]), request.source_head);
        request.current_head = request.source_head.clone();
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        let result = repo
            .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "up_to_date");
    }

    #[tokio::test]
    async fn merges_remote_tracking_branch_with_git_default_message() {
        let (f, mut request) = Fixture::new("feature", false);
        f.git(&[
            "update-ref",
            "refs/remotes/origin/feature",
            &request.source_head,
        ]);
        request.source_ref = "refs/remotes/origin/feature".into();
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        let result = repo
            .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(
            f.git(&["show", "-s", "--format=%s", "HEAD"]),
            "Merge remote-tracking branch 'origin/feature'"
        );
    }

    #[tokio::test]
    async fn squash_creates_one_parent_commit_and_conflicts_preserve_state() {
        let (f, mut request) = Fixture::new("feature", false);
        request.squash = true;
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        let result = repo
            .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_eq!(
            f.git(&["rev-list", "--parents", "-1", "HEAD"])
                .split_whitespace()
                .count(),
            2
        );
        assert_eq!(f.git(&["show", "HEAD:source"]), "source");
        for squash in [false, true] {
            let (f, mut request) = Fixture::new("conflicting", true);
            request.squash = squash;
            let repo = Repository::open(&f.0).await.unwrap();
            let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
            let result = repo
                .renderer_merge_start(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(result["outcome"], "conflict");
            assert!(!f.git(&["ls-files", "--unmerged"]).is_empty());
            assert_eq!(f.git(&["rev-parse", "HEAD"]), request.current_head);
            assert!(
                f.0.join(if squash {
                    ".git/SQUASH_MSG"
                } else {
                    ".git/MERGE_HEAD"
                })
                .exists()
            );
        }
    }

    #[tokio::test]
    async fn rejects_stale_ambiguous_or_unsafe_context() {
        let (f, request) = Fixture::new("feature", false);
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        fs::write(f.0.join("file"), "local\n").unwrap();
        assert!(
            repo.renderer_merge_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["reset", "--hard", "-q", "HEAD"]);
        let preview = repo.renderer_merge_start_preview(&request).await.unwrap();
        f.git(&["update-ref", "refs/heads/feature", &request.current_head]);
        assert!(
            repo.renderer_merge_start(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&["update-ref", "refs/heads/feature", &request.source_head]);
        f.git(&["config", "core.warnAmbiguousRefs", "false"]);
        f.git(&["tag", "feature", &request.source_head]);
        assert!(repo.renderer_merge_start_preview(&request).await.is_err());
        f.git(&["tag", "-d", "feature"]);
        fs::write(f.0.join(".git/SQUASH_MSG"), "pending\n").unwrap();
        assert!(repo.renderer_merge_start_preview(&request).await.is_err());
        let mut invalid = request.clone();
        invalid.source_ref = "--help".into();
        assert!(repo.renderer_merge_start_preview(&invalid).await.is_err());
    }

    #[tokio::test]
    async fn rejects_existing_unmerged_index_without_an_operation_marker() {
        let (f, request) = Fixture::new("conflicting", true);
        let out = Command::new("git")
            .arg("-C")
            .arg(&f.0)
            .args(["merge", "conflicting"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        fs::remove_file(f.0.join(".git/MERGE_HEAD")).unwrap();
        let repo = Repository::open(&f.0).await.unwrap();
        assert!(repo.operation_state().await.unwrap().is_none());
        assert!(repo.renderer_merge_start_preview(&request).await.is_err());
    }

    #[tokio::test]
    async fn squash_refuses_to_commit_preexisting_staged_changes() {
        let (f, mut request) = Fixture::new("feature", false);
        request.squash = true;
        fs::write(f.0.join("unrelated"), "user data\n").unwrap();
        f.git(&["add", "unrelated"]);
        let repo = Repository::open(&f.0).await.unwrap();
        assert!(repo.renderer_merge_start_preview(&request).await.is_err());
        assert_eq!(f.git(&["rev-parse", "HEAD"]), request.current_head);
        assert_eq!(f.git(&["diff", "--cached", "--name-only"]), "unrelated");
    }
}
