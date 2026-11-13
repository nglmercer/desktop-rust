//! Reviewed native revert for the upstream renderer.
//!
//! This fences Desktop's review against its own concurrent operations, but
//! native Git does not reserve the index and ref across the review boundary.
//! External writers may still race it; callers must inspect state on failure.
use crate::{Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Review {
    basis: String,
    head: String,
    branch: Option<String>,
    commit: String,
    parents: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("desktop-revert-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.git(&["init", "-q", "-b", "main"]);
            f.git(&["config", "user.name", "Revert Test"]);
            f.git(&["config", "user.email", "revert@example.invalid"]);
            f.git(&["config", "commit.gpgsign", "false"]);
            f.write("file", "base\n");
            f.git(&["add", "--all"]);
            f.git(&["commit", "-qm", "base"]);
            f
        }
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        }
        fn write(&self, name: &str, contents: &str) {
            fs::write(self.0.join(name), contents).unwrap();
        }
        fn commit(&self, message: &str) -> String {
            self.git(&["add", "--all"]);
            self.git(&["commit", "-qm", message]);
            self.git(&["rev-parse", "HEAD"])
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reverts_selected_commit_and_runs_native_post_commit_hook() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        f.write("file", "changed\n");
        let commit = f.commit("change");
        let hook = f.0.join(".git/hooks/post-commit");
        fs::write(&hook, "#!/bin/sh\nprintf hook >hook-observed\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let repo = Repository::open(&f.0).await.unwrap();
        let review = repo.renderer_revert_preview(&commit).await.unwrap();
        assert_eq!(review["head"], commit);
        assert_eq!(review["mainline"], Value::Null);
        let result = repo
            .renderer_revert(&commit, review["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert_ne!(result["head_before"], result["head_after"]);
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "base\n");
        assert_eq!(
            fs::read_to_string(f.0.join("hook-observed")).unwrap(),
            "hook"
        );
    }

    #[tokio::test]
    async fn rejects_stale_review_without_touching_index_or_head() {
        let f = Fixture::new();
        f.write("file", "changed\n");
        let commit = f.commit("change");
        let repo = Repository::open(&f.0).await.unwrap();
        let review = repo.renderer_revert_preview(&commit).await.unwrap();
        f.write("untracked", "new\n");
        f.git(&["add", "untracked"]);
        let index = fs::read(f.0.join(".git/index")).unwrap();
        assert!(
            repo.renderer_revert(&commit, review["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), commit);
        assert_eq!(fs::read(f.0.join(".git/index")).unwrap(), index);
        f.write("file", "local-one\n");
        let review = repo.renderer_revert_preview(&commit).await.unwrap();
        let status = f.git(&["status", "--porcelain=v1"]);
        f.write("file", "local-two\n");
        assert_eq!(f.git(&["status", "--porcelain=v1"]), status);
        assert!(
            repo.renderer_revert(&commit, review["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(f.git(&["rev-parse", "HEAD"]), commit);
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "local-two\n");
        for invalid in ["HEAD", "--help", "", &"0".repeat(40), &"g".repeat(40)] {
            assert!(repo.renderer_revert_preview(invalid).await.is_err());
        }
    }

    #[tokio::test]
    async fn reports_conflict_and_retains_native_abort_state() {
        let f = Fixture::new();
        f.write("file", "selected\n");
        let target = f.commit("selected");
        f.write("file", "newer\n");
        let head = f.commit("newer");
        let repo = Repository::open(&f.0).await.unwrap();
        let review = repo.renderer_revert_preview(&target).await.unwrap();
        let result = repo
            .renderer_revert(&target, review["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "conflict");
        assert_eq!(result["head_after"], head);
        assert_eq!(f.git(&["rev-parse", "REVERT_HEAD"]), target);
        assert!(!f.git(&["ls-files", "--unmerged"]).is_empty());
        f.git(&["revert", "--abort"]);
        assert_eq!(fs::read_to_string(f.0.join("file")).unwrap(), "newer\n");
        assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    }

    #[tokio::test]
    async fn merge_revert_selects_first_parent() {
        let f = Fixture::new();
        f.git(&["switch", "-qc", "side"]);
        f.write("side", "side\n");
        f.commit("side");
        f.git(&["switch", "-q", "main"]);
        f.write("main", "main\n");
        f.commit("main");
        f.git(&["merge", "--no-ff", "-qm", "merge", "side"]);
        let merge = f.git(&["rev-parse", "HEAD"]);
        let repo = Repository::open(&f.0).await.unwrap();
        let review = repo.renderer_revert_preview(&merge).await.unwrap();
        assert_eq!(review["mainline"], 1);
        let result = repo
            .renderer_revert(&merge, review["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["outcome"], "complete");
        assert!(!f.0.join("side").exists());
        assert_eq!(fs::read_to_string(f.0.join("main")).unwrap(), "main\n");
    }
}

impl Repository {
    async fn renderer_revert_review(&self, commit: &str) -> Result<Review> {
        // The review uses shared repository readers which inherit the process
        // environment. Refuse overrides so it describes the same repository
        // and index as the bound mutation below.
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
                "Cannot review a revert with {variable} set"
            );
        }
        anyhow::ensure!(
            matches!(commit.len(), 40 | 64)
                && commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "A complete commit object ID is required"
        );
        anyhow::ensure!(
            self.resolve_commit(commit).await? == commit,
            "Commit no longer resolves to the selected object"
        );
        anyhow::ensure!(
            self.operation_state().await?.is_none(),
            "Finish or abort the current Git operation before reverting"
        );
        let git_dir = self.git_directory().await?;
        match std::fs::symlink_metadata(git_dir.join("SQUASH_MSG")) {
            Ok(_) => anyhow::bail!("Finish or abort the pending squash before reverting"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let head = self
            .run(&args(&[
                "rev-parse",
                "--verify",
                "--end-of-options",
                "HEAD^{commit}",
            ]))
            .await?;
        anyhow::ensure!(head.code == 0, "A commit is required before reverting");
        let head = String::from_utf8(head.stdout)?.trim().to_owned();
        anyhow::ensure!(
            head.len() == commit.len() && head.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid current HEAD"
        );
        let branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(branch.code, 0 | 1),
            "Cannot inspect the current branch"
        );
        let branch = (branch.code == 0)
            .then(|| String::from_utf8(branch.stdout))
            .transpose()?
            .map(|v| v.trim_end_matches('\n').to_owned());
        if let Some(reference) = &branch {
            anyhow::ensure!(
                reference.starts_with("refs/heads/") && !reference.contains(['\0', '\n', '\r']),
                "Invalid current branch"
            );
        }
        let parents = self
            .run(&args(&[
                "rev-list",
                "--parents",
                "--max-count=1",
                commit,
                "--",
            ]))
            .await?;
        anyhow::ensure!(parents.code == 0, "Cannot inspect the selected commit");
        let parent_line = std::str::from_utf8(&parents.stdout)?.trim_end();
        let mut fields = parent_line.split_whitespace();
        anyhow::ensure!(
            fields.next() == Some(commit),
            "Invalid selected commit metadata"
        );
        let parents = fields.collect::<Vec<_>>();
        anyhow::ensure!(
            parents
                .iter()
                .all(|p| p.len() == commit.len() && p.bytes().all(|b| b.is_ascii_hexdigit())),
            "Invalid commit parent"
        );
        // Reuse the checkout certificate: it covers exact changed-file bytes,
        // nested repositories, the index, attributes, config and operation
        // state instead of trusting the status letters alone.
        let capture = self.checkout_capture(0).await?;
        let expected_branch = branch
            .as_deref()
            .and_then(|v| v.strip_prefix("refs/heads/"));
        anyhow::ensure!(
            capture["snapshot"]["head"].as_str() == Some(&head)
                && capture["snapshot"]["branch"].as_str() == expected_branch
                && capture["snapshot"]["operation"].is_null(),
            "Repository changed while reviewing the revert"
        );
        let captured = serde_json::to_vec(&capture)?;
        let mut hash = Sha256::new();
        fn field(hash: &mut Sha256, bytes: &[u8]) {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        for bytes in [
            self.id().as_bytes(),
            commit.as_bytes(),
            head.as_bytes(),
            branch.as_deref().unwrap_or("").as_bytes(),
            &captured,
        ] {
            field(&mut hash, bytes);
        }
        Ok(Review {
            basis: format!("{:x}", hash.finalize()),
            head,
            branch,
            commit: commit.into(),
            parents: parents.len(),
        })
    }

    pub(crate) async fn renderer_revert_preview(&self, commit: &str) -> Result<Value> {
        let review = self.renderer_revert_review(commit).await?;
        Ok(
            json!({"basis":review.basis,"head":review.head,"branch":review.branch,"commit":review.commit,"mainline":(review.parents > 1).then_some(1)}),
        )
    }

    pub(crate) async fn renderer_revert(&self, commit: &str, basis: &str) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        let review = self.renderer_revert_review(commit).await?;
        anyhow::ensure!(
            review.basis == basis,
            "Revert context changed; refresh and review the commit again"
        );
        let mut command = args(&["revert", "--no-edit"]);
        if review.parents > 1 {
            command.extend(args(&["-m", "1"]));
        }
        command.push(review.commit.clone().into());
        let output = process::run_bound_checkout(&self.path, &command).await?;
        let revert_head = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "REVERT_HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(revert_head.code, 0 | 1),
            "Cannot inspect revert outcome; inspect HEAD and REVERT_HEAD before retrying"
        );
        let marker = if revert_head.code == 0 {
            Some(std::str::from_utf8(&revert_head.stdout)?.trim().to_owned())
        } else {
            None
        };
        anyhow::ensure!(
            marker.as_deref().is_none_or(|head| head == review.commit),
            "Revert state changed; inspect HEAD and REVERT_HEAD before retrying"
        );
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(unmerged.code == 0, "Cannot inspect revert conflicts");
        let sequencer_revert = self
            .operation_state()
            .await?
            .is_some_and(|state| state.kind == "revert");
        let conflict = output.code != 0
            && !unmerged.stdout.is_empty()
            && (marker.as_deref() == Some(&review.commit)
                || (marker.is_none() && sequencer_revert));
        let current = self
            .run(&args(&["rev-parse", "--verify", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            current.code == 0,
            "Cannot inspect revert outcome; inspect HEAD and REVERT_HEAD before retrying"
        );
        let new_head = String::from_utf8(current.stdout)?.trim().to_owned();
        let current_branch = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(
            matches!(current_branch.code, 0 | 1),
            "Cannot inspect current branch after revert"
        );
        let current_branch = if current_branch.code == 0 {
            Some(
                String::from_utf8(current_branch.stdout)?
                    .trim_end_matches('\n')
                    .to_owned(),
            )
        } else {
            None
        };
        anyhow::ensure!(
            current_branch == review.branch,
            "Branch changed during revert; inspect the repository before retrying"
        );
        let result = if output.code == 0 {
            anyhow::ensure!(
                marker.is_none() && !sequencer_revert && new_head != review.head,
                "Revert outcome is uncertain; inspect HEAD and REVERT_HEAD before retrying"
            );
            let parents = self
                .run(&args(&[
                    "rev-list",
                    "--parents",
                    "--max-count=1",
                    &new_head,
                    "--",
                ]))
                .await?;
            anyhow::ensure!(
                parents.code == 0
                    && std::str::from_utf8(&parents.stdout)?
                        .split_whitespace()
                        .nth(1)
                        == Some(&review.head),
                "Revert outcome is uncertain; inspect commit history before retrying"
            );
            "complete"
        } else if conflict {
            anyhow::ensure!(
                new_head == review.head,
                "Revert outcome is uncertain; inspect HEAD and REVERT_HEAD before retrying"
            );
            "conflict"
        } else {
            anyhow::ensure!(
                new_head == review.head,
                "HEAD changed during failed revert; inspect the repository before retrying"
            );
            "failed"
        };
        Ok(
            json!({"outcome":result,"code":output.code,"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),"head_before":review.head,"head_after":new_head,"commit":review.commit}),
        )
    }
}
