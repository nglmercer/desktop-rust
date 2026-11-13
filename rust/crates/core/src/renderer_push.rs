//! Reviewed branch and tag publication. External writers and remote changes can
//! race Git after the final review; an interrupted push must not be retried
//! without inspecting the destination.
use crate::{RendererPushRequest, Repository, args, process};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    // The existing UI expects hook lifecycle and an interactive decision on
    // failure. The reviewed native transport cannot supply that decision yet.
    // Refuse before mutation whenever Git may execute a pre-push hook.
    async fn push_requires_legacy_hooks(&self, request: &RendererPushRequest) -> Result<bool> {
        if !request.require_hook_callbacks || request.no_verify {
            return Ok(false);
        }
        let configured = self
            .run(&args(&["config", "--null", "--get-regexp", "^hook\\."]))
            .await?;
        ensure!(
            matches!(configured.code, 0 | 1),
            "Cannot inspect configured Git hooks"
        );
        if configured.code == 0 {
            return Ok(true);
        }
        let path = self
            .run(&args(&["rev-parse", "--git-path", "hooks/pre-push"]))
            .await?;
        ensure!(path.code == 0, "Cannot inspect pre-push hook");
        let raw = path
            .stdout
            .strip_suffix(b"\n")
            .ok_or_else(|| anyhow::anyhow!("Invalid pre-push hook path"))?;
        ensure!(
            !raw.is_empty() && !raw.contains(&0),
            "Invalid pre-push hook path"
        );
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStrExt;
            std::path::PathBuf::from(std::ffi::OsStr::from_bytes(raw))
        };
        #[cfg(not(unix))]
        let path = std::path::PathBuf::from(std::str::from_utf8(raw)?);
        let path = if path.is_absolute() {
            path
        } else {
            self.path.join(path)
        };
        match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(_) => anyhow::bail!("Cannot safely inspect pre-push hook"),
        }
    }

    async fn push_destinations(&self, remote: &str) -> Result<Vec<String>> {
        let output = self
            .run(&args(&[
                "remote", "get-url", "--push", "--all", "--", remote,
            ]))
            .await?;
        ensure!(output.code == 0, "Cannot inspect push destinations");
        let urls = std::str::from_utf8(&output.stdout)?
            .strip_suffix('\n')
            .ok_or_else(|| anyhow::anyhow!("Invalid push destinations"))?;
        let urls: Vec<String> = urls.split('\n').map(str::to_owned).collect();
        ensure!(
            !urls.is_empty()
                && urls.len() <= 32
                && urls
                    .iter()
                    .all(|url| crate::remote_discovery::safe_url(url)),
            "Push destinations cannot be safely reviewed"
        );
        Ok(urls)
    }

    async fn push_reference(&self, namespace: &str, name: &str, exists: bool) -> Result<String> {
        ensure!(
            !name.is_empty() && name.len() <= 4096 && !name.starts_with('-'),
            "Invalid push reference"
        );
        let reference = format!("refs/{namespace}/{name}");
        let checked = self.run(&args(&["check-ref-format", &reference])).await?;
        ensure!(checked.code == 0, "Invalid push reference");
        if exists {
            let target = self
                .run(&args(&["show-ref", "--verify", "--hash", "--", &reference]))
                .await?;
            ensure!(target.code == 0, "Push reference no longer exists");
        }
        Ok(reference)
    }

    async fn renderer_push_review(
        &self,
        request: &RendererPushRequest,
    ) -> Result<(String, Vec<String>)> {
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
            ensure!(
                std::env::var_os(variable).is_none(),
                "Cannot review push with {variable} set"
            );
        }
        ensure!(
            !request.remote.is_empty()
                && request.remote.len() <= 4096
                && !request.remote.chars().any(char::is_control),
            "Invalid configured remote"
        );
        let remote_ref = format!("refs/remotes/{}/placeholder", request.remote);
        ensure!(
            self.run(&args(&["check-ref-format", &remote_ref]))
                .await?
                .code
                == 0,
            "Invalid configured remote"
        );
        ensure!(
            self.fetch_remote_names().await?.contains(&request.remote),
            "Configured remote no longer exists"
        );
        ensure!(
            crate::remote_discovery::safe_url(&request.expected_url),
            "Remote URL cannot be safely reviewed"
        );
        let fetch_url = self
            .run(&args(&[
                "-c",
                "transfer.credentialsInUrl=allow",
                "ls-remote",
                "--get-url",
                "--",
                &request.remote,
            ]))
            .await?;
        ensure!(
            fetch_url.code == 0
                && fetch_url.stdout.strip_suffix(b"\n") == Some(request.expected_url.as_bytes()),
            "Remote changed; refresh before pushing"
        );
        let destinations = self.push_destinations(&request.remote).await?;
        self.push_reference("heads", &request.local_branch, true)
            .await?;
        self.push_reference(
            "heads",
            request
                .remote_branch
                .as_deref()
                .unwrap_or(&request.local_branch),
            false,
        )
        .await?;
        ensure!(request.tags.len() <= 4096, "Too many tags to push");
        let mut tags = std::collections::HashSet::new();
        for name in &request.tags {
            ensure!(tags.insert(name), "Duplicate push tag");
            self.push_reference("tags", name, true).await?;
        }
        let refs = self
            .run(&args(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(symref)",
            ]))
            .await?;
        ensure!(refs.code == 0, "Cannot inspect repository refs");
        let config = self.checkout_configuration().await?;
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &serde_json::to_vec(&destinations)?,
            &refs.stdout,
            &config,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok((format!("{:x}", hash.finalize()), destinations))
    }

    pub(crate) async fn renderer_push_preview(
        &self,
        request: &RendererPushRequest,
    ) -> Result<Value> {
        if !crate::remote_discovery::safe_url(&request.expected_url) {
            return Ok(json!({"kind":"unsupported"}));
        }
        if self.push_requires_legacy_hooks(request).await? {
            return Ok(json!({"kind":"unsupported_hooks"}));
        }
        let (basis, destinations) = self.renderer_push_review(request).await?;
        Ok(json!({"request":request, "destinations":destinations, "basis":basis}))
    }

    pub(crate) async fn renderer_push(
        &self,
        request: &RendererPushRequest,
        basis: &str,
    ) -> Result<Value> {
        let _lock = self.stash_lock().await?;
        ensure!(
            !self.push_requires_legacy_hooks(request).await?,
            "Pre-push hook changed; refresh before continuing"
        );
        ensure!(
            self.renderer_push_review(request).await?.0 == basis,
            "Push context changed; refresh before continuing"
        );
        let source = self
            .push_reference("heads", &request.local_branch, true)
            .await?;
        let destination = self
            .push_reference(
                "heads",
                request
                    .remote_branch
                    .as_deref()
                    .unwrap_or(&request.local_branch),
                false,
            )
            .await?;
        let mut command = args(&["push"]);
        if request.remote_branch.is_none() {
            command.push("--set-upstream".into());
        } else if request.force_with_lease {
            command.push("--force-with-lease".into());
        }
        if request.no_verify {
            command.push("--no-verify".into());
        }
        if request.report_progress {
            command.push("--progress".into());
        }
        command.extend(args(&[
            "--",
            &request.remote,
            &format!("{source}:{destination}"),
        ]));
        for name in &request.tags {
            let tag = self.push_reference("tags", name, true).await?;
            command.push(format!("{tag}:{tag}").into());
        }
        ensure!(
            !self.push_requires_legacy_hooks(request).await?,
            "Pre-push hook changed; refresh before continuing"
        );
        let output =
            process::run_bound_remote_fetch(&self.path, &command, false, request.report_progress)
                .await?;
        Ok(
            json!({"code":output.code, "error_kind":if output.code == 0 { None } else { crate::push_error::classify(&output.stderr, &output.stdout) }}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    fn git(dir: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> (Self, RendererPushRequest) {
            let root = std::env::temp_dir().join(format!("desktop-push-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("source")).unwrap();
            fs::create_dir(root.join("remote.git")).unwrap();
            let src = root.join("source");
            git(&src, &["init", "-q", "-b", "main"]);
            git(&root.join("remote.git"), &["init", "--bare", "-q"]);
            git(&src, &["config", "user.name", "Test"]);
            git(&src, &["config", "user.email", "test@example.invalid"]);
            git(&src, &["config", "commit.gpgsign", "false"]);
            fs::write(src.join("file"), "base\n").unwrap();
            git(&src, &["add", "file"]);
            git(&src, &["commit", "-qm", "base"]);
            git(&src, &["tag", "v1"]);
            let url = root.join("remote.git").to_string_lossy().into_owned();
            git(&src, &["remote", "add", "origin", &url]);
            let request = RendererPushRequest {
                remote: "origin".into(),
                expected_url: url,
                local_branch: "main".into(),
                remote_branch: None,
                tags: vec!["v1".into()],
                force_with_lease: false,
                no_verify: false,
                report_progress: false,
                require_hook_callbacks: false,
            };
            (Self(root), request)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn pushes_branch_and_selected_tag_and_sets_upstream() {
        let (fixture, request) = Fixture::new();
        let src = fixture.0.join("source");
        let repo = Repository::open(&src).await.unwrap();
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let result = repo
            .renderer_push(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        let commit = git(&src, &["rev-parse", "HEAD"]);
        assert_eq!(
            git(
                &fixture.0.join("remote.git"),
                &["rev-parse", "refs/heads/main"]
            ),
            commit
        );
        assert_eq!(
            git(
                &fixture.0.join("remote.git"),
                &["rev-parse", "refs/tags/v1"]
            ),
            commit
        );
        assert_eq!(
            git(
                &src,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}"
                ]
            ),
            "origin/main"
        );
    }

    #[tokio::test]
    async fn changed_push_destination_or_branch_ref_rejects_stale_review() {
        let (fixture, request) = Fixture::new();
        let src = fixture.0.join("source");
        let repo = Repository::open(&src).await.unwrap();
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        git(
            &src,
            &["config", "remote.origin.pushurl", "/tmp/new-destination"],
        );
        assert!(
            repo.renderer_push(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        git(&src, &["config", "--unset", "remote.origin.pushurl"]);
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        fs::write(src.join("file"), "new\n").unwrap();
        git(&src, &["add", "file"]);
        git(&src, &["commit", "-qm", "new"]);
        assert!(
            repo.renderer_push(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        let output = Command::new("git")
            .current_dir(fixture.0.join("remote.git"))
            .args(["show-ref", "--verify", "refs/heads/main"])
            .output()
            .unwrap();
        assert!(!output.status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reviews_each_destination_and_reports_partial_success() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, request) = Fixture::new();
        let src = fixture.0.join("source");
        let first = fixture.0.join("remote.git");
        let second = fixture.0.join("second.git");
        fs::create_dir(&second).unwrap();
        git(&second, &["init", "--bare", "-q"]);
        let hook = second.join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        git(
            &src,
            &[
                "config",
                "--add",
                "remote.origin.pushurl",
                first.to_str().unwrap(),
            ],
        );
        git(
            &src,
            &[
                "config",
                "--add",
                "remote.origin.pushurl",
                second.to_str().unwrap(),
            ],
        );
        let repo = Repository::open(&src).await.unwrap();
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        assert_eq!(
            preview["destinations"],
            json!([first.to_str().unwrap(), second.to_str().unwrap()])
        );
        let result = repo
            .renderer_push(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_ne!(result["code"], 0);
        assert_eq!(
            git(&first, &["rev-parse", "refs/heads/main"]),
            git(&src, &["rev-parse", "HEAD"])
        );
        let missing = Command::new("git")
            .current_dir(&second)
            .args(["show-ref", "--verify", "refs/heads/main"])
            .output()
            .unwrap();
        assert!(!missing.status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failing_hook_prevents_push_unless_user_explicitly_disables_it() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        let hook = src.join(".git/hooks/pre-push");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        let repo = Repository::open(&src).await.unwrap();
        request.require_hook_callbacks = true;
        assert_eq!(
            repo.renderer_push_preview(&request).await.unwrap()["kind"],
            "unsupported_hooks"
        );
        request.require_hook_callbacks = false;
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let result = repo
            .renderer_push(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_ne!(result["code"], 0);
        request.no_verify = true;
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let result = repo
            .renderer_push(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
    }

    #[tokio::test]
    async fn rejects_a_new_hook_after_review() {
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        request.require_hook_callbacks = true;
        let repo = Repository::open(&src).await.unwrap();
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        fs::write(src.join(".git/hooks/pre-push"), "#!/bin/sh\nexit 1\n").unwrap();
        assert!(
            repo.renderer_push(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        assert_eq!(
            repo.renderer_push_preview(&request).await.unwrap()["kind"],
            "unsupported_hooks"
        );
    }
}
