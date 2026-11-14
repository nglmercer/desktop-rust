//! Reviewed Git pull execution for the shared renderer.
//!
//! Pull can update tracking refs, the index and the worktree even on failure.
//! Never automatically retry a failed or cancelled invocation.
use crate::{RendererFetchRequest, Repository, args, process};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// Never return Git's diagnostics: they can contain paths, credentials, or
// remote-controlled text. Match only local Git diagnostic prefixes.
fn classify_pull_failure(
    stderr: &[u8],
    stdout: &[u8],
    owner: Option<&str>,
) -> Option<&'static str> {
    for stream in [stderr, stdout] {
        let text = String::from_utf8_lossy(stream);
        if text
            .lines()
            .any(|line| line.starts_with("Your configuration specifies to merge with the ref '"))
            && text
                .lines()
                .any(|line| line == "from the remote, but no such ref was fetched.")
        {
            return Some("no_existing_remote_branch");
        }
        for line in text.lines() {
            if line == "There are no candidates for merging among the refs that you just fetched."
                || line
                    == "There are no candidates for rebasing among the refs that you just fetched."
            {
                return Some("no_matching_remote_branch");
            }
            if line == "There is no tracking information for the current branch."
                || (line.starts_with("You asked to pull from the remote '")
                    && line.ends_with("', but did not specify"))
            {
                return Some("missing_upstream");
            }
            if line == "fatal: refusing to merge unrelated histories" {
                return Some("unrelated_histories");
            }
            if line.starts_with("error: could not lock config file ")
                && line.ends_with(": File exists")
            {
                return Some("config_lock_exists");
            }
            if line
                == "error: Your local changes to the following files would be overwritten by merge:"
            {
                return Some("merge_local_changes");
            }
            if line
                == "error: The following untracked working tree files would be overwritten by merge:"
                || line
                    == "error: Your local changes to the following files would be overwritten by checkout:"
                || line
                    == "error: The following untracked working tree files would be overwritten by checkout:"
            {
                return Some("local_changes_overwritten");
            }
            if line.starts_with("error: cannot pull with rebase: You have unstaged changes.")
                || line.starts_with(
                    "error: cannot pull with rebase: Your index contains uncommitted changes.",
                )
                || line.starts_with("error: cannot rebase: You have unstaged changes.")
            {
                return Some("rebase_local_changes");
            }
        }
    }
    crate::fetch_error::classify(stderr, stdout, owner)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RendererPullRequest {
    pub remote: String,
    pub expected_url: String,
    pub no_verify: bool,
    pub report_progress: bool,
    #[serde(default)]
    pub require_hook_callbacks: bool,
    #[serde(default)]
    pub expected_saml_owner: Option<String>,
}

impl std::fmt::Debug for RendererPullRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RendererPullRequest")
            .field("remote", &self.remote)
            .field("expected_url", &"<redacted>")
            .field("no_verify", &self.no_verify)
            .field("report_progress", &self.report_progress)
            .field("require_hook_callbacks", &self.require_hook_callbacks)
            .finish()
    }
}

impl Repository {
    async fn pull_has_submodules(&self) -> Result<bool> {
        // A recursive pull may fetch with the parent's credential environment.
        // Refuse before mutation until nested account scoping has been proven.
        let index = self.run(&args(&["ls-files", "--stage", "-z"])).await?;
        ensure!(index.code == 0, "Cannot inspect repository submodules");
        if index
            .stdout
            .split(|byte| *byte == 0)
            .any(|entry| entry.starts_with(b"160000 "))
        {
            return Ok(true);
        }
        let config = self
            .run(&args(&["config", "--get-regexp", "^submodule\\."]))
            .await?;
        ensure!(
            matches!(config.code, 0 | 1),
            "Cannot inspect submodule configuration"
        );
        Ok(config.code == 0)
    }

    async fn renderer_pull_review(&self, request: &RendererPullRequest) -> Result<(String, bool)> {
        ensure!(
            !request.remote.starts_with('-'),
            "Cannot pull from a remote whose name starts with '-'"
        );
        ensure!(
            !self.pull_has_submodules().await?,
            "Recursive pull with submodules is unavailable"
        );
        ensure!(
            self.operation_state().await?.is_none(),
            "Finish the current Git operation before pulling"
        );
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        ensure!(
            unmerged.code == 0 && unmerged.stdout.is_empty(),
            "Resolve existing index conflicts before pulling"
        );
        let fetch = RendererFetchRequest {
            remote: request.remote.clone(),
            expected_url: request.expected_url.clone(),
            recurse_submodules: true,
            is_background_task: false,
            report_progress: request.report_progress,
            expected_saml_owner: request.expected_saml_owner.clone(),
            refspec: None,
        };
        let fetch_basis = self.renderer_fetch_review(&fetch).await?;
        let head = self
            .run(&args(&["symbolic-ref", "--quiet", "HEAD"]))
            .await?;
        ensure!(head.code == 0, "Pull requires a checked-out branch");
        let commit = self
            .run(&args(&["rev-parse", "--verify", "HEAD^{commit}"]))
            .await?;
        ensure!(commit.code == 0, "Pull requires an existing commit");
        let pull_ff = self.run(&args(&["config", "--get", "pull.ff"])).await?;
        // Match the renderer: use --ff only when pull.ff is absent. For any
        // other config error, let Git interpret its existing configuration.
        let use_default_ff = pull_ff.code == 1;
        let mut hash = Sha256::new();
        for part in [
            fetch_basis.as_bytes(),
            &head.stdout,
            &commit.stdout,
            &pull_ff.stdout,
            &pull_ff.code.to_le_bytes(),
            &serde_json::to_vec(request)?,
        ] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Ok((format!("{:x}", hash.finalize()), use_default_ff))
    }

    /// Review the selected remote and checked-out branch before pulling.
    pub async fn renderer_pull_preview(&self, request: &RendererPullRequest) -> Result<Value> {
        if !crate::remote_discovery::safe_url(&request.expected_url) {
            return Ok(json!({"kind":"unsupported"}));
        }
        // The legacy hook environment may load a user-configured shell and
        // needs interactive failure callbacks. It is not yet implemented by
        // this route, so refuse before any fetch or worktree mutation.
        if request.require_hook_callbacks {
            return Ok(json!({"kind":"unsupported_hooks"}));
        }
        match self.pull_has_submodules().await {
            Ok(true) => return Ok(json!({"kind":"unsupported_submodules"})),
            Ok(false) => {}
            // A bounded index inspection may fail for very large repositories.
            // Fall back before mutating instead of bypassing the review.
            Err(_) => return Ok(json!({"kind":"unsupported"})),
        }
        if self.operation_state().await?.is_some() {
            return Ok(json!({"kind":"unsupported"}));
        }
        let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        ensure!(unmerged.code == 0, "Cannot inspect index conflicts");
        if !unmerged.stdout.is_empty() {
            return Ok(json!({"kind":"unsupported"}));
        }
        let (basis, _) = self.renderer_pull_review(request).await?;
        Ok(json!({"remote": request.remote, "basis": basis}))
    }

    /// Execute one Git pull, preserving configured merge/rebase behavior.
    /// The configured hook environment and interactive callbacks are not yet
    /// supported; callers requesting them are refused before mutation.
    pub async fn renderer_pull(&self, request: &RendererPullRequest, basis: &str) -> Result<Value> {
        ensure!(
            !request.require_hook_callbacks,
            "Interactive pull hooks are unavailable"
        );
        let _lock = self.stash_lock().await?;
        let (current, use_default_ff) = self.renderer_pull_review(request).await?;
        ensure!(
            current == basis,
            "Pull context changed; refresh before continuing"
        );
        let mut command = args(&["-c", "rebase.backend=merge", "pull"]);
        if use_default_ff {
            command.push("--ff".into());
        }
        command.push("--recurse-submodules".into());
        if request.report_progress {
            command.push("--progress".into());
        }
        if request.no_verify {
            command.push("--no-verify".into());
        }
        command.extend(args(&["--", &request.remote]));
        let output =
            process::run_bound_remote_fetch(&self.path, &command, false, request.report_progress)
                .await?;
        let error_kind = if output.code == 0 {
            None
        } else {
            let unmerged = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
            if unmerged.code == 0 && !unmerged.stdout.is_empty() {
                match self.operation_state().await?.map(|state| state.kind) {
                    Some(kind) if kind == "merge" => Some("merge_conflicts"),
                    Some(kind) if kind == "rebase" => Some("rebase_conflicts"),
                    _ => classify_pull_failure(
                        &output.stderr,
                        &output.stdout,
                        request.expected_saml_owner.as_deref(),
                    ),
                }
            } else {
                classify_pull_failure(
                    &output.stderr,
                    &output.stdout,
                    request.expected_saml_owner.as_deref(),
                )
            }
        };
        Ok(json!({"code": output.code, "error_kind": error_kind}))
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

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    // Re-execute each fixture in a private process: Repository's Git runner
    // inherits process environment and global Git configuration. Changing
    // those globals inside a parallel Rust test would affect unrelated tests.
    fn isolated(name: &str) -> bool {
        const MARKER: &str = "DESKTOP_PULL_FIXTURE_CASE";
        if std::env::var(MARKER).ok().as_deref() == Some(name) {
            return false;
        }
        let root = std::env::temp_dir().join(format!("desktop-pull-env-{}", uuid::Uuid::new_v4()));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        fs::create_dir_all(root.join("templates")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            &format!("renderer_pull::tests::{name}"),
            "--nocapture",
        ]);
        child.current_dir(&root);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                child.env_remove(key);
            }
        }
        child.env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
        child
            .env("HOME", &root)
            .env("USERPROFILE", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("TMPDIR", &root)
            .env("GIT_CONFIG_GLOBAL", root.join("empty-gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TEMPLATE_DIR", root.join("templates"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env(MARKER, name);
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "isolated {name} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("desktop-pull-{}", uuid::Uuid::new_v4()));
            let source = root.join("source");
            let target = root.join("target");
            fs::create_dir_all(&source).unwrap();
            git(&source, &["init", "-q", "-b", "main", "--template="]);
            for args in [
                ["user.name", "Test"],
                ["user.email", "test@example.invalid"],
                ["commit.gpgsign", "false"],
            ] {
                git(&source, &["config", args[0], args[1]]);
            }
            fs::write(source.join("base"), "base\n").unwrap();
            git(&source, &["add", "base"]);
            git(&source, &["commit", "-qm", "base"]);
            git(
                &root,
                &[
                    "clone",
                    "-q",
                    "--no-local",
                    source.to_str().unwrap(),
                    target.to_str().unwrap(),
                ],
            );
            for args in [
                ["user.name", "Test"],
                ["user.email", "test@example.invalid"],
                ["commit.gpgsign", "false"],
            ] {
                git(&target, &["config", args[0], args[1]]);
            }
            Self(root)
        }
        fn source(&self) -> PathBuf {
            self.0.join("source")
        }
        fn target(&self) -> PathBuf {
            self.0.join("target")
        }
        fn request(&self) -> RendererPullRequest {
            RendererPullRequest {
                remote: "origin".into(),
                expected_url: self.source().to_str().unwrap().into(),
                no_verify: false,
                report_progress: false,
                require_hook_callbacks: false,
                expected_saml_owner: None,
            }
        }
        fn commit(&self, directory: &Path, name: &str, contents: &str) {
            fs::write(directory.join(name), contents).unwrap();
            git(directory, &["add", "--", name]);
            git(directory, &["commit", "-qm", name]);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn reviewed_pull_fast_forwards_and_rejects_stale_review() {
        if isolated("reviewed_pull_fast_forwards_and_rejects_stale_review") {
            return;
        }
        let f = Fixture::new();
        f.commit(&f.source(), "remote", "remote\n");
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        let basis = preview["basis"].as_str().unwrap();
        git(&f.target(), &["config", "pull.ff", "only"]);
        assert!(repo.renderer_pull(&request, basis).await.is_err());
        assert!(!f.target().join("remote").exists());
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        let result = repo
            .renderer_pull(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert_eq!(
            git(&f.target(), &["rev-parse", "HEAD"]),
            git(&f.source(), &["rev-parse", "HEAD"])
        );
    }

    #[tokio::test]
    async fn reviewed_pull_respects_merge_and_rebase_configuration() {
        if isolated("reviewed_pull_respects_merge_and_rebase_configuration") {
            return;
        }
        for rebase in [false, true] {
            let f = Fixture::new();
            f.commit(&f.source(), "remote", "remote\n");
            f.commit(&f.target(), "local", "local\n");
            let remote_head = git(&f.source(), &["rev-parse", "HEAD"]);
            git(
                &f.target(),
                &[
                    "config",
                    "pull.rebase",
                    if rebase { "true" } else { "false" },
                ],
            );
            let repo = Repository::open(&f.target()).await.unwrap();
            let request = f.request();
            let preview = repo.renderer_pull_preview(&request).await.unwrap();
            let result = repo
                .renderer_pull(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(result["code"], 0);
            assert_eq!(
                fs::read_to_string(f.target().join("local")).unwrap(),
                "local\n"
            );
            assert_eq!(
                fs::read_to_string(f.target().join("remote")).unwrap(),
                "remote\n"
            );
            let parents = git(&f.target(), &["rev-list", "--parents", "-n", "1", "HEAD"]);
            let parents: Vec<_> = parents.split_whitespace().collect();
            if rebase {
                assert_eq!(parents.len(), 2);
                assert_eq!(parents[1], remote_head);
            } else {
                assert_eq!(parents.len(), 3);
                assert_eq!(parents[2], remote_head);
            }
        }
    }

    #[tokio::test]
    async fn reviewed_pull_preserves_conflicts_and_does_not_retry() {
        if isolated("reviewed_pull_preserves_conflicts_and_does_not_retry") {
            return;
        }
        let f = Fixture::new();
        f.commit(&f.source(), "base", "remote\n");
        f.commit(&f.target(), "base", "local\n");
        git(&f.target(), &["config", "pull.rebase", "false"]);
        let old = git(&f.target(), &["rev-parse", "HEAD"]);
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        let result = repo
            .renderer_pull(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_ne!(result["code"], 0);
        assert_eq!(result["error_kind"], "merge_conflicts");
        assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), old);
        assert!(!git(&f.target(), &["diff", "--name-only", "--diff-filter=U"]).is_empty());
        assert!(f.target().join(".git/MERGE_HEAD").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reviewed_pull_runs_merge_hook_once_or_respects_explicit_no_verify() {
        if isolated("reviewed_pull_runs_merge_hook_once_or_respects_explicit_no_verify") {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        for no_verify in [false, true] {
            let f = Fixture::new();
            f.commit(&f.source(), "remote", "remote\n");
            f.commit(&f.target(), "local", "local\n");
            git(&f.target(), &["config", "pull.rebase", "false"]);
            let hooks = f.target().join(".git/hooks");
            fs::create_dir_all(&hooks).unwrap();
            let hook = hooks.join("pre-merge-commit");
            let marker = f.0.join("hook-runs");
            fs::write(
                &hook,
                format!(
                    "#!/bin/sh\nprintf 'run\\n' >> '{}'\nexit 3\n",
                    marker.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let old = git(&f.target(), &["rev-parse", "HEAD"]);
            let repo = Repository::open(&f.target()).await.unwrap();
            let mut request = f.request();
            request.no_verify = no_verify;
            let preview = repo.renderer_pull_preview(&request).await.unwrap();
            let result = repo
                .renderer_pull(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            if no_verify {
                assert_eq!(result["code"], 0);
                assert!(!marker.exists());
                assert_ne!(git(&f.target(), &["rev-parse", "HEAD"]), old);
            } else {
                assert_ne!(result["code"], 0);
                assert_eq!(fs::read_to_string(marker).unwrap(), "run\n");
                assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), old);
            }
        }
    }

    #[tokio::test]
    async fn interactive_hook_environment_is_refused_before_any_fetch() {
        if isolated("interactive_hook_environment_is_refused_before_any_fetch") {
            return;
        }
        let f = Fixture::new();
        f.commit(&f.source(), "remote", "remote\n");
        let old = git(&f.target(), &["rev-parse", "HEAD"]);
        let repo = Repository::open(&f.target()).await.unwrap();
        let mut request = f.request();
        request.require_hook_callbacks = true;
        assert_eq!(
            repo.renderer_pull_preview(&request).await.unwrap()["kind"],
            "unsupported_hooks"
        );
        assert!(repo.renderer_pull(&request, "unused").await.is_err());
        assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), old);
        assert!(!f.target().join("remote").exists());
        // No hook scan can change this result: callbacks remain unavailable,
        // including when a hook is installed after the unsupported preview.
        let hooks = f.target().join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-merge-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        assert!(repo.renderer_pull(&request, "unused").await.is_err());
    }

    #[tokio::test]
    async fn unavailable_remote_returns_a_fixed_classification() {
        if isolated("unavailable_remote_returns_a_fixed_classification") {
            return;
        }
        let f = Fixture::new();
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        fs::remove_dir_all(f.source()).unwrap();
        let result = repo
            .renderer_pull(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_ne!(result["code"], 0);
        assert_eq!(result["error_kind"], "remote_repository_unavailable");
        assert!(!result.to_string().contains(f.0.to_str().unwrap()));
    }

    #[tokio::test]
    async fn configured_or_indexed_submodules_are_refused_before_fetch() {
        if isolated("configured_or_indexed_submodules_are_refused_before_fetch") {
            return;
        }
        let f = Fixture::new();
        f.commit(&f.source(), "remote", "remote\n");
        let original = git(&f.target(), &["rev-parse", "HEAD"]);
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        git(
            &f.target(),
            &[
                "config",
                "submodule.child.url",
                "https://other.invalid/child",
            ],
        );
        assert_eq!(
            repo.renderer_pull_preview(&request).await.unwrap()["kind"],
            "unsupported_submodules"
        );
        assert!(
            repo.renderer_pull(&request, preview["basis"].as_str().unwrap())
                .await
                .is_err()
        );
        git(&f.target(), &["config", "--unset", "submodule.child.url"]);
        git(
            &f.target(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{original},nested"),
            ],
        );
        assert_eq!(
            repo.renderer_pull_preview(&request).await.unwrap()["kind"],
            "unsupported_submodules"
        );
        assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), original);
        assert!(!f.target().join("remote").exists());
    }

    #[tokio::test]
    async fn incoming_new_submodule_remains_uninitialized() {
        if isolated("incoming_new_submodule_remains_uninitialized") {
            return;
        }
        let f = Fixture::new();
        let nested = f.0.join("nested-source");
        fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main", "--template="]);
        git(&nested, &["config", "user.name", "Test"]);
        git(&nested, &["config", "user.email", "test@example.invalid"]);
        f.commit(&nested, "file", "nested\n");
        git(
            &f.source(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                nested.to_str().unwrap(),
                "child",
            ],
        );
        git(&f.source(), &["commit", "-qm", "add child"]);
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        let result = repo
            .renderer_pull(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(result["code"], 0);
        assert!(f.target().join("child").is_dir());
        assert_eq!(fs::read_dir(f.target().join("child")).unwrap().count(), 0);
        assert!(!f.target().join(".git/modules/child").exists());
    }

    #[tokio::test]
    async fn classifies_real_local_obstructions_and_unrelated_histories() {
        if isolated("classifies_real_local_obstructions_and_unrelated_histories") {
            return;
        }
        for case in ["merge", "untracked", "rebase", "unrelated"] {
            let f = Fixture::new();
            match case {
                "merge" | "rebase" => {
                    f.commit(&f.source(), "base", "remote\n");
                    fs::write(f.target().join("base"), "uncommitted\n").unwrap();
                    git(
                        &f.target(),
                        &[
                            "config",
                            "pull.rebase",
                            if case == "rebase" { "true" } else { "false" },
                        ],
                    );
                }
                "untracked" => {
                    f.commit(&f.source(), "collision", "remote\n");
                    fs::write(f.target().join("collision"), "untracked\n").unwrap();
                }
                _ => {
                    git(&f.source(), &["checkout", "-q", "--orphan", "unrelated"]);
                    git(&f.source(), &["rm", "-q", "-rf", "."]);
                    f.commit(&f.source(), "other", "other\n");
                    git(&f.source(), &["branch", "-M", "main"]);
                }
            }
            let old = git(&f.target(), &["rev-parse", "HEAD"]);
            let repo = Repository::open(&f.target()).await.unwrap();
            let request = f.request();
            let preview = repo.renderer_pull_preview(&request).await.unwrap();
            let result = repo
                .renderer_pull(&request, preview["basis"].as_str().unwrap())
                .await
                .unwrap();
            assert_ne!(result["code"], 0, "{case}");
            let expected = match case {
                "merge" => "merge_local_changes",
                "untracked" => "local_changes_overwritten",
                "rebase" => "rebase_local_changes",
                _ => "unrelated_histories",
            };
            assert_eq!(result["error_kind"], expected, "{case}");
            assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), old);
            assert!(!result.to_string().contains(f.0.to_str().unwrap()));
        }
    }

    #[tokio::test]
    async fn classifies_real_git_configuration_lock_without_exposing_path() {
        if isolated("classifies_real_git_configuration_lock_without_exposing_path") {
            return;
        }
        let f = Fixture::new();
        fs::write(f.target().join(".git/config.lock"), "locked").unwrap();
        let output = Command::new("git")
            .current_dir(f.target())
            .args(["config", "--local", "test.key", "value"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(
            classify_pull_failure(&output.stderr, &output.stdout, None),
            Some("config_lock_exists")
        );
    }

    #[tokio::test]
    async fn reports_a_missing_upstream_without_exposing_git_diagnostics() {
        if isolated("reports_a_missing_upstream_without_exposing_git_diagnostics") {
            return;
        }
        let f = Fixture::new();
        git(&f.target(), &["config", "--unset", "branch.main.merge"]);
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        let preview = repo.renderer_pull_preview(&request).await.unwrap();
        let result = repo
            .renderer_pull(&request, preview["basis"].as_str().unwrap())
            .await
            .unwrap();
        assert_ne!(result["code"], 0);
        assert_eq!(result["error_kind"], "missing_upstream");
        assert!(!result.to_string().contains(f.0.to_str().unwrap()));
    }

    #[tokio::test]
    async fn oversized_index_review_is_refused_before_fetch() {
        use std::io::Write;
        use std::process::Stdio;
        if isolated("oversized_index_review_is_refused_before_fetch") {
            return;
        }
        let f = Fixture::new();
        f.commit(&f.source(), "remote", "remote\n");
        let old = git(&f.target(), &["rev-parse", "HEAD"]);
        let blob = git(&f.target(), &["rev-parse", "HEAD:base"]);
        let mut update = Command::new("git")
            .current_dir(f.target())
            .args(["update-index", "--index-info"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let stdin = update.stdin.as_mut().unwrap();
            let padding = "x".repeat(80);
            for i in 0..20_000 {
                writeln!(stdin, "100644 {blob}\tentry-{i:05}-{padding}").unwrap();
            }
        }
        drop(update.stdin.take());
        let output = update.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let repo = Repository::open(&f.target()).await.unwrap();
        let request = f.request();
        assert_eq!(
            repo.renderer_pull_preview(&request).await.unwrap()["kind"],
            "unsupported"
        );
        assert_eq!(git(&f.target(), &["rev-parse", "HEAD"]), old);
        assert!(!f.target().join("remote").exists());
    }
}
