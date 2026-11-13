//! Reviewed local tracking-branch fast-forwards. Git owns ref/worktree locks;
//! independent writers can still race the review.
use crate::{RendererFastForwardRequest, Repository, args, process};
use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

impl Repository {
    async fn tracking_refs(&self) -> Result<HashMap<String, (String, String, String)>> {
        let output = self
            .run(&args(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(upstream)%00%(symref)",
                "refs/heads/",
                "refs/remotes/",
            ]))
            .await?;
        anyhow::ensure!(output.code == 0, "Cannot inspect branch references");
        let mut refs = HashMap::new();
        for line in output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
        {
            let fields = line.split(|b| *b == 0).collect::<Vec<_>>();
            anyhow::ensure!(fields.len() == 4, "Invalid branch reference response");
            let name = std::str::from_utf8(fields[0])?.to_owned();
            let value = (
                std::str::from_utf8(fields[1])?.into(),
                std::str::from_utf8(fields[2])?.into(),
                std::str::from_utf8(fields[3])?.into(),
            );
            anyhow::ensure!(
                refs.insert(name, value).is_none(),
                "Duplicate Git reference response"
            );
        }
        Ok(refs)
    }

    async fn renderer_fast_forward_review(
        &self,
        request: &RendererFastForwardRequest,
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
                "Cannot review branch updates with {variable} set"
            );
        }
        anyhow::ensure!(
            !request.branches.is_empty() && request.branches.len() <= 4096,
            "A bounded list of tracking branches is required"
        );
        let input_size = request.branches.iter().try_fold(0usize, |size, branch| {
            anyhow::ensure!(
                branch.reference.len() <= 4096 && branch.upstream_ref.len() <= 4096,
                "Branch reference is too long"
            );
            size.checked_add(branch.reference.len())
                .and_then(|n| n.checked_add(branch.upstream_ref.len()))
                .and_then(|n| n.checked_add(2))
                .ok_or_else(|| anyhow::anyhow!("Branch refspec input is too large"))
        })?;
        anyhow::ensure!(
            input_size <= 1024 * 1024,
            "Branch refspec input is too large"
        );
        let mut destinations = HashSet::new();
        let oid = |value: &str| {
            matches!(value.len(), 40 | 64)
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let head = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        anyhow::ensure!(matches!(head.code, 0 | 1), "Cannot inspect current branch");
        let current = if head.code == 0 {
            Some(
                std::str::from_utf8(&head.stdout)?
                    .trim_end_matches('\n')
                    .to_owned(),
            )
        } else {
            None
        };
        let refs = self.tracking_refs().await?;
        for branch in &request.branches {
            anyhow::ensure!(
                branch.reference.starts_with("refs/heads/")
                    && branch.upstream_ref.starts_with("refs/remotes/")
                    && destinations.insert(branch.reference.clone()),
                "Invalid or duplicate tracking branch"
            );
            anyhow::ensure!(
                oid(&branch.sha)
                    && oid(&branch.upstream_sha)
                    && branch.sha.len() == branch.upstream_sha.len(),
                "Complete branch object IDs are required"
            );
            anyhow::ensure!(
                current.as_deref() != Some(&branch.reference),
                "Cannot fast-forward the currently checked-out branch"
            );
            for reference in [&branch.reference, &branch.upstream_ref] {
                anyhow::ensure!(
                    !reference
                        .bytes()
                        .any(|b| b <= b' ' || b == 127 || b":?*[\\".contains(&b)),
                    "Invalid branch reference"
                );
            }
            let (sha, upstream, symbolic) = refs
                .get(&branch.reference)
                .ok_or_else(|| anyhow::anyhow!("Selected branch no longer exists"))?;
            let (upstream_sha, _, upstream_symbolic) = refs
                .get(&branch.upstream_ref)
                .ok_or_else(|| anyhow::anyhow!("Selected upstream no longer exists"))?;
            anyhow::ensure!(
                *sha == branch.sha
                    && *upstream == branch.upstream_ref
                    && symbolic.is_empty()
                    && *upstream_sha == branch.upstream_sha
                    && upstream_symbolic.is_empty(),
                "Tracking branch changed; refresh before updating"
            );
        }
        let configuration = self.checkout_configuration().await?;
        let worktrees = self.worktrees().await?;
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &configuration,
            &serde_json::to_vec(&worktrees)?,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) async fn renderer_fast_forward_preview(
        &self,
        request: &RendererFastForwardRequest,
    ) -> Result<Value> {
        Ok(json!({"request":request,"basis":self.renderer_fast_forward_review(request).await?}))
    }

    pub(crate) async fn renderer_fast_forward(
        &self,
        request: &RendererFastForwardRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        anyhow::ensure!(
            self.renderer_fast_forward_review(request).await? == basis,
            "Tracking branches changed; refresh before updating"
        );
        let mut command = args(&[
            "fetch",
            ".",
            "--show-forced-updates",
            "--no-write-fetch-head",
        ]);
        if !request.recurse_submodules {
            command.extend(args(&["--recurse-submodules=no"]));
        }
        command.extend(args(&["--stdin"]));
        let input = request
            .branches
            .iter()
            .map(|branch| format!("{}:{}\n", branch.upstream_ref, branch.reference))
            .collect::<String>()
            .into_bytes();
        let output = process::run_bound_local_fetch(&self.path, &command, input).await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "Branch fast-forward failed; inspect refs before retrying"
        );
        Ok(json!({"code":output.code}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RendererTrackingBranch;
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
        fn new() -> (Self, RendererTrackingBranch) {
            let root = std::env::temp_dir().join(format!("desktop-ff-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let f = Self(root);
            f.git(&["init", "-q", "-b", "main"]);
            f.git(&["config", "user.name", "Test"]);
            f.git(&["config", "user.email", "test@example.invalid"]);
            f.git(&["config", "commit.gpgsign", "false"]);
            fs::write(f.0.join("file"), "base\n").unwrap();
            f.git(&["add", "file"]);
            f.git(&["commit", "-qm", "base"]);
            let sha = f.git(&["rev-parse", "HEAD"]);
            f.git(&["branch", "tracking", &sha]);
            fs::write(f.0.join("file"), "upstream\n").unwrap();
            f.git(&["commit", "-qam", "upstream"]);
            let upstream_sha = f.git(&["rev-parse", "HEAD"]);
            f.git(&[
                "config",
                "remote.origin.url",
                "https://example.invalid/repo",
            ]);
            f.git(&[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ]);
            f.git(&["config", "branch.tracking.remote", "origin"]);
            f.git(&["config", "branch.tracking.merge", "refs/heads/topic"]);
            f.git(&["update-ref", "refs/remotes/origin/topic", &upstream_sha]);
            (
                f,
                RendererTrackingBranch {
                    reference: "refs/heads/tracking".into(),
                    sha,
                    upstream_ref: "refs/remotes/origin/topic".into(),
                    upstream_sha,
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
    async fn advances_tracking_ref_without_rewriting_fetch_head() {
        let (f, branch) = Fixture::new();
        f.git(&["config", "fetch.prune", "true"]);
        f.git(&["update-ref", "refs/remotes/origin/unrelated", &branch.sha]);
        fs::write(f.0.join(".git/FETCH_HEAD"), "preserve me\n").unwrap();
        let request = RendererFastForwardRequest {
            branches: vec![branch],
            recurse_submodules: false,
        };
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_fast_forward_preview(&request).await.unwrap();
        let result = repo
            .renderer_fast_forward(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(
            f.git(&["rev-parse", "refs/heads/tracking"]),
            request.branches[0].upstream_sha
        );
        assert_eq!(
            fs::read_to_string(f.0.join(".git/FETCH_HEAD")).unwrap(),
            "preserve me\n"
        );
        assert_eq!(
            f.git(&["rev-parse", "refs/remotes/origin/unrelated"]),
            request.branches[0].sha
        );
        assert!(
            f.git(&["reflog", "-1", "--format=%gs", "tracking"])
                .starts_with("pull")
        );
    }

    #[tokio::test]
    async fn refuses_changed_refs_tracking_configuration_and_option_injection() {
        let (f, branch) = Fixture::new();
        let request = RendererFastForwardRequest {
            branches: vec![branch],
            recurse_submodules: false,
        };
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_fast_forward_preview(&request).await.unwrap();
        f.git(&[
            "update-ref",
            "refs/remotes/origin/topic",
            &request.branches[0].sha,
        ]);
        assert!(
            repo.renderer_fast_forward(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        f.git(&[
            "update-ref",
            "refs/remotes/origin/topic",
            &request.branches[0].upstream_sha,
        ]);
        let preview = repo.renderer_fast_forward_preview(&request).await.unwrap();
        f.git(&["config", "branch.tracking.merge", "refs/heads/other"]);
        assert!(
            repo.renderer_fast_forward(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        let mut invalid = request.clone();
        invalid.branches[0].reference = "refs/heads/a:refs/heads/b".into();
        assert!(repo.renderer_fast_forward_preview(&invalid).await.is_err());
        assert_eq!(
            f.git(&["rev-parse", "refs/heads/tracking"]),
            request.branches[0].sha
        );
    }

    #[tokio::test]
    async fn non_fast_forward_is_partial_and_preserves_other_successes() {
        let (f, branch) = Fixture::new();
        f.git(&["switch", "-qc", "diverged", &branch.sha]);
        fs::write(f.0.join("other"), "diverged\n").unwrap();
        f.git(&["add", "other"]);
        f.git(&["commit", "-qm", "diverged"]);
        let divergent_sha = f.git(&["rev-parse", "HEAD"]);
        f.git(&["switch", "-q", "main"]);
        f.git(&["config", "branch.diverged.remote", "origin"]);
        f.git(&["config", "branch.diverged.merge", "refs/heads/topic"]);
        let divergent = RendererTrackingBranch {
            reference: "refs/heads/diverged".into(),
            sha: divergent_sha.clone(),
            upstream_ref: branch.upstream_ref.clone(),
            upstream_sha: branch.upstream_sha.clone(),
        };
        let request = RendererFastForwardRequest {
            branches: vec![branch, divergent],
            recurse_submodules: false,
        };
        let repo = Repository::open(&f.0).await.unwrap();
        let preview = repo.renderer_fast_forward_preview(&request).await.unwrap();
        let result = repo
            .renderer_fast_forward(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 1);
        assert_eq!(f.git(&["rev-parse", "refs/heads/diverged"]), divergent_sha);
        assert_eq!(
            f.git(&["rev-parse", "refs/heads/tracking"]),
            request.branches[0].upstream_sha
        );
    }

    #[tokio::test]
    async fn refuses_current_branch_and_does_not_update_a_linked_worktree_branch() {
        let (f, branch) = Fixture::new();
        let repo = Repository::open(&f.0).await.unwrap();
        let mut invalid = branch.clone();
        invalid.reference = "refs/heads/main".into();
        invalid.sha = invalid.upstream_sha.clone();
        assert!(
            repo.renderer_fast_forward_preview(&RendererFastForwardRequest {
                branches: vec![invalid],
                recurse_submodules: false
            })
            .await
            .is_err()
        );
        let linked =
            std::env::temp_dir().join(format!("desktop-ff-linked-{}", uuid::Uuid::new_v4()));
        let request = RendererFastForwardRequest {
            branches: vec![branch],
            recurse_submodules: false,
        };
        let stale = repo.renderer_fast_forward_preview(&request).await.unwrap();
        f.git(&[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "tracking",
        ]);
        assert!(
            repo.renderer_fast_forward(&request, stale["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        let preview = repo.renderer_fast_forward_preview(&request).await.unwrap();
        let result = repo
            .renderer_fast_forward(&request, preview["basis"].as_str().unwrap())
            .await;
        if let Ok(value) = result {
            assert_eq!(value["code"], 1);
        }
        assert_eq!(
            f.git(&["rev-parse", "refs/heads/tracking"]),
            request.branches[0].sha
        );
        f.git(&["worktree", "remove", linked.to_str().unwrap()]);
    }
}
