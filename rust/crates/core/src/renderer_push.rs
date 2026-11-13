//! Reviewed branch and tag publication. External writers and remote changes can
//! race Git after the final review; an interrupted push must not be retried
//! without inspecting the destination.
use crate::{RendererPushRequest, Repository, args, process};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl Repository {
    async fn pre_push_hook_path(&self) -> Result<std::path::PathBuf> {
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
        Ok(if path.is_absolute() {
            path
        } else {
            self.path.join(path)
        })
    }

    /// Bind a reviewed push to the pre-push file Git would run. Linked hooks
    /// are supported, but replacing or modifying the resolved file is refused.
    #[cfg(unix)]
    async fn pre_push_hook_fingerprint(&self) -> Result<Vec<u8>> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let path = self.pre_push_hook_path().await?;
        let link = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(b"missing".to_vec());
            }
            Err(_) => anyhow::bail!("Cannot review pre-push hook"),
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| anyhow::anyhow!("Cannot review pre-push hook"))?;
        let before = file.metadata()?;
        ensure!(
            before.is_file() && before.len() <= 1024 * 1024,
            "Pre-push hook cannot be safely reviewed"
        );
        let mut contents = Vec::new();
        (&file).take(1024 * 1024 + 1).read_to_end(&mut contents)?;
        ensure!(
            contents.len() <= 1024 * 1024,
            "Pre-push hook cannot be safely reviewed"
        );
        let after = file.metadata()?;
        let current = std::fs::metadata(&path)?;
        let same = |a: &std::fs::Metadata, b: &std::fs::Metadata| {
            a.dev() == b.dev()
                && a.ino() == b.ino()
                && a.mode() == b.mode()
                && a.len() == b.len()
                && a.mtime() == b.mtime()
                && a.mtime_nsec() == b.mtime_nsec()
                && a.ctime() == b.ctime()
                && a.ctime_nsec() == b.ctime_nsec()
        };
        ensure!(
            same(&before, &after) && same(&after, &current),
            "Pre-push hook changed during review"
        );
        let mut hash = Sha256::new();
        hash.update(link.mode().to_le_bytes());
        hash.update(before.dev().to_le_bytes());
        hash.update(before.ino().to_le_bytes());
        hash.update(before.mode().to_le_bytes());
        hash.update(contents);
        Ok(hash.finalize().to_vec())
    }

    // Preserve a user-supplied Trace2 destination. Our private Trace2 pipe is
    // used only when neither environment nor Git config already selects one.
    async fn push_hook_trace_available(&self, request: &RendererPushRequest) -> Result<bool> {
        if !request.require_hook_callbacks || request.no_verify {
            return Ok(false);
        }
        #[cfg(not(unix))]
        return Ok(false);
        #[cfg(unix)]
        {
            if std::env::var_os("GIT_TRACE2_EVENT").is_some() {
                return Ok(false);
            }
            let configured = self
                .run(&args(&["config", "--get", "trace2.eventTarget"]))
                .await?;
            ensure!(
                matches!(configured.code, 0 | 1),
                "Cannot inspect Git trace configuration"
            );
            Ok(configured.code == 1)
        }
    }

    // When native hook progress cannot be delivered, retain the old fail-closed
    // behavior before starting a push that could invoke a hook.
    async fn push_requires_legacy_hooks(&self, request: &RendererPushRequest) -> Result<bool> {
        if !request.require_hook_callbacks || request.no_verify {
            return Ok(false);
        }
        // Configured hooks may refer to external commands and multiple hook
        // invocations that cannot yet be bound to this review. Keep them
        // blocked even when the traditional hook can be traced safely.
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
        if self.push_hook_trace_available(request).await? {
            return Ok(false);
        }
        let path = self.pre_push_hook_path().await?;
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
        #[cfg(unix)]
        let hook = if request.require_hook_callbacks && !request.no_verify {
            self.pre_push_hook_fingerprint().await?
        } else {
            Vec::new()
        };
        #[cfg(not(unix))]
        let hook = Vec::<u8>::new();
        let mut hash = Sha256::new();
        for part in [
            self.id().as_bytes(),
            &serde_json::to_vec(request)?,
            &serde_json::to_vec(&destinations)?,
            &refs.stdout,
            &config,
            &hook,
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
        let trace_hooks = self.push_hook_trace_available(request).await?;
        let (output, hook_failed) = process::run_bound_remote_push(
            &self.path,
            &command,
            request.report_progress,
            trace_hooks,
        )
        .await?;
        Ok(
            json!({"code":output.code, "hook_failed":hook_failed, "error_kind":if output.code == 0 { None } else { crate::push_error::classify(&output.stderr, &output.stdout) }}),
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
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let result = crate::hook_progress::scope(
            sender,
            17,
            repo.renderer_push(&request, preview["basis"].as_str().unwrap()),
        )
        .await
        .unwrap();
        assert_ne!(result["code"], 0);
        let mut statuses = Vec::new();
        while let Ok((id, event)) = receiver.try_recv() {
            if let crate::hook_progress::CommitEvent::Hook(hook) = event {
                assert_eq!(id, 17);
                assert_eq!(hook.hook_name, "pre-push");
                statuses.push(hook.status);
            }
        }
        assert_eq!(statuses, ["started", "failed"]);
        request.no_verify = true;
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let result = repo
            .renderer_push(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_pre_push_hook_is_executed_and_reported() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        let hook = src.join(".git/hooks/pre-push");
        fs::write(
            &hook,
            "#!/bin/sh\ncat > hook-input\nprintf '%s\\n' \"$1\" > hook-remote\n",
        )
        .unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        request.require_hook_callbacks = true;
        let repo = Repository::open(&src).await.unwrap();
        let preview = repo.renderer_push_preview(&request).await.unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let result = crate::hook_progress::scope(
            sender,
            7,
            repo.renderer_push(&request, preview["basis"].as_str().unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(
            fs::read_to_string(src.join("hook-remote")).unwrap(),
            "origin\n"
        );
        assert!(
            fs::read_to_string(src.join("hook-input"))
                .unwrap()
                .contains("refs/heads/main")
        );
        let mut statuses = Vec::new();
        while let Ok((_, event)) = receiver.try_recv() {
            if let crate::hook_progress::CommitEvent::Hook(hook) = event {
                statuses.push(hook.status);
            }
        }
        assert_eq!(statuses, ["started", "finished"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preserves_a_user_configured_trace_destination() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        let hook = src.join(".git/hooks/pre-push");
        fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let trace = fixture.0.join("user-trace.json");
        git(
            &src,
            &["config", "trace2.eventTarget", trace.to_str().unwrap()],
        );
        request.require_hook_callbacks = true;
        let repo = Repository::open(&src).await.unwrap();
        assert_eq!(
            repo.renderer_push_preview(&request).await.unwrap()["kind"],
            "unsupported_hooks"
        );
        assert_eq!(
            git(&src, &["config", "--get", "trace2.eventTarget"]),
            trace.to_str().unwrap()
        );
    }

    #[tokio::test]
    async fn rejects_configured_hooks_that_cannot_be_bound_to_review() {
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        git(&src, &["config", "hook.first.command", "/tmp/first-hook"]);
        git(&src, &["config", "hook.first.event", "pre-push"]);
        git(&src, &["config", "hook.second.command", "/tmp/second-hook"]);
        git(&src, &["config", "hook.second.event", "pre-push"]);
        request.require_hook_callbacks = true;
        let repo = Repository::open(&src).await.unwrap();
        assert_eq!(
            repo.renderer_push_preview(&request).await.unwrap()["kind"],
            "unsupported_hooks"
        );
        let remote = fixture.0.join("remote.git");
        assert!(
            !Command::new("git")
                .current_dir(remote)
                .args(["show-ref", "--verify", "refs/heads/main"])
                .output()
                .unwrap()
                .status
                .success()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_during_pre_push_stops_the_hook_before_publication() {
        use std::os::unix::fs::PermissionsExt;
        let (fixture, mut request) = Fixture::new();
        let src = fixture.0.join("source");
        let hook = src.join(".git/hooks/pre-push");
        fs::write(&hook, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
        request.require_hook_callbacks = true;
        let repo = Repository::open(&src).await.unwrap();
        let basis = repo.renderer_push_preview(&request).await.unwrap()["basis"]
            .as_str()
            .unwrap()
            .to_owned();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let cancellation = crate::cancellation::Cancellation::default();
        let task = tokio::spawn(cancellation.clone().scope(crate::hook_progress::scope(
            sender,
            8,
            async move { repo.renderer_push(&request, &basis).await },
        )));
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while let Some((_, event)) = receiver.recv().await {
                if matches!(event, crate::hook_progress::CommitEvent::Hook(ref hook) if hook.hook_name == "pre-push" && hook.status == "started") { return; }
            }
            panic!("hook never started");
        }).await.unwrap();
        cancellation.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        let status = Command::new("git")
            .current_dir(fixture.0.join("remote.git"))
            .args(["show-ref", "--verify", "refs/heads/main"])
            .status()
            .unwrap();
        assert!(!status.success());
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
        assert!(repo.renderer_push_preview(&request).await.unwrap()["basis"].is_string());
    }
}
