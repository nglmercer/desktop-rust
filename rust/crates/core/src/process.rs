use crate::CommandOutput;
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub struct RawOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl RawOutput {
    /// Match the legacy Git error classification while keeping other failures distinct.
    pub(crate) fn bad_revision(&self) -> bool {
        self.code != 0
            && self
                .stderr
                .split(|byte| *byte == b'\n')
                .any(|line| line.starts_with(b"fatal: bad revision '") && line.ends_with(b"'"))
    }

    pub fn text(self) -> CommandOutput {
        CommandOutput {
            code: self.code,
            stdout: String::from_utf8_lossy(&self.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&self.stderr).into_owned(),
        }
    }
}

pub async fn run(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_git(path, args, None, None).await
}

/// Stop a blob producer as soon as its output exceeds the preview budget.
pub(crate) async fn run_resolution_blob(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command_inner(
        path,
        git_native_paths(None),
        args,
        None,
        crate::comparison_snapshots::MAX_BYTES as u64,
        None,
        Some(OutputSlice::BoundedBlob),
        None,
        None,
    )
    .await
}

/// Apply Desktop's clone transport policy after URL rewrites, including when
/// an inherited allowlist would otherwise override Git's protocol settings.
pub(crate) async fn run_clone(
    path: &Path,
    args: &[OsString],
    report_progress: bool,
) -> Result<RawOutput> {
    run_clone_inner(path, args, report_progress, &[]).await
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) async fn run_clone_with_inherited_fd(
    path: &Path,
    args: &[OsString],
    report_progress: bool,
    handle: &std::fs::File,
    home: Option<&std::fs::File>,
) -> Result<RawOutput> {
    use std::os::fd::AsRawFd;
    let mut descriptors = vec![handle.as_raw_fd()];
    if let Some(home) = home { descriptors.push(home.as_raw_fd()); }
    anyhow::ensure!(descriptors.iter().all(|fd| *fd >= 3), "Clone directory descriptor overlaps a standard stream");
    run_clone_inner(path, args, report_progress, &descriptors).await
}

async fn run_clone_inner(
    path: &Path,
    args: &[OsString],
    report_progress: bool,
    inherited_fds: &[i32],
) -> Result<RawOutput> {
    let mut command = git_native_paths(None);
    #[cfg(unix)]
    if !inherited_fds.is_empty() {
        #[cfg(test)]
        {
            let fd = inherited_fds[0];
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd, metadata.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let metadata = unsafe { metadata.assume_init() };
            command.env("DESKTOP_TEST_CLONE_CHILD_FD", fd.to_string())
                .env("DESKTOP_TEST_CLONE_CHILD_IDENTITY", format!("{}:{}", metadata.st_dev, metadata.st_ino));
            if let Some(home_fd) = inherited_fds.get(1) {
                let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe { libc::fstat(*home_fd, metadata.as_mut_ptr()) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let metadata = unsafe { metadata.assume_init() };
                command.env("DESKTOP_TEST_CLONE_HOME_IDENTITY", format!("{}:{}", metadata.st_dev, metadata.st_ino));
            }
        }
        // This runs after fork, so clearing CLOEXEC changes only the child's
        // descriptor table. The parent descriptor remains private.
        let descriptors = inherited_fds.to_vec();
        unsafe {
            command.pre_exec(move || {
                for fd in &descriptors {
                    let flags = libc::fcntl(*fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = inherited_fds;
    // Fixed diagnostics allow safe classification without forwarding Git's
    // potentially credential-bearing error output to the caller.
    command.env("LC_ALL", "C");
    if crate::git_credentials::clone::active() {
        command.env("GIT_ALLOW_PROTOCOL", "https");
    } else if let Some(value) = std::env::var_os("GIT_ALLOW_PROTOCOL") {
        let value = value
            .into_string()
            .map_err(|_| anyhow::anyhow!("Invalid Git protocol allowlist"))?;
        let filtered = value
            .split(':')
            .filter(|protocol| {
                !protocol.eq_ignore_ascii_case("ext") && !protocol.eq_ignore_ascii_case("ext.exe")
            })
            .collect::<Vec<_>>()
            .join(":");
        command.env("GIT_ALLOW_PROTOCOL", filtered);
    }
    let sender = report_progress
        .then(crate::hook_progress::sender)
        .flatten();
    let emit_clone_progress = sender.is_some();
    let (sender, request_id) = sender.unwrap_or_else(|| {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        (sender, 0)
    });
    // Clone always uses draining capture, even if no progress receiver exists.
    let live = Some(LiveOutput {
        sender,
        request_id,
        checkpoint: None,
        clone_progress: true,
        emit_clone_progress,
    });
    run_command_inner(
        path,
        command,
        args,
        None,
        2 * 1024 * 1024,
        None,
        None,
        None,
        live,
    )
    .await
}

pub(crate) async fn repository_discovery(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    let mut command = git_command(None);
    command.env("LC_ALL", "C");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

pub(crate) async fn configuration(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    crate::git_credentials::native_configuration(run(path, args)).await
}

pub async fn run_git(
    path: &Path,
    args: &[OsString],
    input: Option<Vec<u8>>,
    index: Option<&Path>,
) -> Result<RawOutput> {
    let command = git_command(index);
    run_command(path, command, args, input, 2 * 1024 * 1024, None).await
}

/// Status lists can be much larger than ordinary command responses. Keep a
/// separate bound so a large worktree does not weaken limits for other Git
/// operations or trigger the legacy status fallback.
pub(crate) async fn run_status(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(path, git_command(None), args, None, 16 * 1024 * 1024, None).await
}

/// Capture a bounded comparison snapshot. The snapshot reservation limits
/// concurrent callers before this allocation starts.
pub(crate) async fn run_comparison_snapshot(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    // Retain at most the render limit plus one byte, but drain the pipe so
    // Git can finish and report its real exit status.
    run_command_inner(
        path,
        git_command(None),
        args,
        None,
        crate::comparison_snapshots::MAX_BYTES as u64 + 1,
        None,
        Some(OutputSlice::PrefixAndDrain),
        None,
        None,
    )
    .await
}

/// Bound staged patch output while draining the Git process to completion.
pub(crate) async fn run_staged_patch(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command_inner(
        path,
        git_command(None),
        args,
        None,
        10 * 1024 * 1024 + 1,
        None,
        Some(OutputSlice::PrefixAndDrain),
        None,
        None,
    )
    .await
}

/// Inspect an index using an isolated, temporary Git directory. Never inherit
/// repository or configuration overrides that could redirect the probe or run
/// a configured fsmonitor command.
pub(crate) async fn inspect_saved_index(
    worktree: &Path,
    git_directory: &Path,
    index: &Path,
) -> Result<RawOutput> {
    let mut command = git_command(Some(index));
    command
        .env("GIT_DIR", git_directory)
        .env("GIT_COMMON_DIR", git_directory)
        .env("GIT_WORK_TREE", worktree)
        .env("GIT_CONFIG_GLOBAL", git_directory.join("empty-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("GIT_NO_LAZY_FETCH", "1");
    run_command(
        worktree,
        command,
        &crate::args(&[
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "--sparse",
            "--stage",
            "-z",
        ]),
        None,
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// Capture at most `limit` bytes from a known blob. Closing stdout after the
/// prefix can make Git exit with SIGPIPE, so the caller must verify the blob
/// beforehand and interpret the exit status together with the expected size.
pub(crate) async fn blob_range(
    path: &Path,
    oid: &str,
    offset: u64,
    limit: u64,
) -> Result<RawOutput> {
    run_command_inner(
        path,
        git_command(None),
        &crate::args(&["cat-file", "blob", oid]),
        None,
        limit,
        None,
        Some(OutputSlice::Range(offset)),
        None,
        None,
    )
    .await
}

/// Parse supplied config bytes without discovering a repository or loading user configuration.
#[cfg(unix)]
pub(crate) async fn parse_git_config(bytes: Vec<u8>) -> Result<RawOutput> {
    let mut command = Command::new("git");
    command
        .env("GIT_DIR", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_TERMINAL_PROMPT", "0");
    run_command(
        &std::env::temp_dir(),
        command,
        &crate::args(&["config", "--file", "-", "--no-includes", "--null", "--list"]),
        Some(bytes),
        2 * 1024 * 1024,
        None,
    )
    .await
}
#[cfg(not(unix))]
pub(crate) async fn parse_git_config(_: Vec<u8>) -> Result<RawOutput> {
    anyhow::bail!("Git configuration cannot be verified on this platform")
}

/// Typed commits contain message options, never pathspec operands. Do not leak
/// our direct-file literal mode into user hooks or Git LFS post-commit hooks.
pub async fn run_commit(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(
        path,
        git_native_paths(None),
        args,
        None,
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// Run a commit against a private index while preserving Git's normal hook
/// pathspec environment. The caller owns index/ref reconciliation and must
/// not treat a successful process exit as proof that publication is durable.
pub(crate) async fn run_commit_with_index(
    path: &Path,
    args: &[OsString],
    input: Vec<u8>,
    index: &Path,
    checkpoint: Option<&str>,
) -> Result<RawOutput> {
    #[cfg(unix)]
    let mut command = git_native_paths(Some(index));
    #[cfg(not(unix))]
    let command = git_native_paths(Some(index));
    #[cfg(unix)]
    let trace_target_unset = if std::env::var_os("GIT_TRACE2_EVENT").is_none()
        && crate::hook_progress::sender().is_some()
    {
        matches!(run(path, &crate::args(&["config", "--get", "trace2.eventTarget"])).await, Ok(output) if output.code == 1)
    } else {
        false
    };
    #[cfg(unix)]
    let trace = if trace_target_unset {
        match (crate::hook_progress::sender(), checkpoint, index.parent()) {
            (Some((sender, request_id)), Some(id), Some(directory)) => {
                crate::hook_trace::Trace::start(directory, id, sender, request_id).ok()
            }
            _ => None,
        }
    } else {
        None
    };
    #[cfg(unix)]
    if let Some(trace) = &trace {
        command.env("GIT_TRACE2_EVENT", trace.path());
    }
    let live =
        crate::hook_progress::sender()
            .zip(checkpoint)
            .map(|((sender, request_id), checkpoint)| LiveOutput {
                sender,
                request_id,
                checkpoint: Some(checkpoint.to_owned()),
                clone_progress: false,
                emit_clone_progress: false,
            });
    let result = Box::pin(run_command_inner(
        path,
        command,
        args,
        Some(input),
        2 * 1024 * 1024,
        None,
        None,
        None,
        live,
    ))
    .await;
    #[cfg(unix)]
    if let Some(trace) = trace {
        trace.finish().await;
    }
    result
}

/// Remote management uses names, not pathspecs; do not pass a literal-path
/// override to any Git hook spawned by the operation.
pub(crate) async fn run_remote(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(
        path,
        git_native_paths(None),
        args,
        None,
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// `update-index --stdin -z` reads literal paths, so filters should inherit
/// Git's normal pathspec environment rather than the direct-file override.
pub(crate) async fn update_index(
    path: &Path,
    args: &[OsString],
    input: Vec<u8>,
) -> Result<RawOutput> {
    run_command(
        path,
        git_native_paths(None),
        args,
        Some(input),
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// Inspect only existing objects; a partial clone must not fetch implicitly.
pub async fn local_commit_available(path: &Path, oid: &str) -> Result<bool> {
    let mut command = git_command(None);
    command.env("GIT_NO_LAZY_FETCH", "1");
    let output = run_command(
        path,
        command,
        &crate::args(&["cat-file", "-e", &format!("{oid}^{{commit}}")]),
        None,
        2 * 1024 * 1024,
        None,
    )
    .await?;
    Ok(output.code == 0)
}

/// Stash commands own their native pathspecs and may invoke LFS internally.
pub async fn run_stash(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(
        path,
        git_native_paths(None),
        args,
        None,
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// Run a destructive repository operation without inherited Git directory overrides.
fn bound_git_command() -> Command {
    let mut command = git_native_paths(None);
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
        command.env_remove(variable);
    }
    command.env("LC_ALL", "C");
    command
}

pub(crate) async fn run_bound_checkout(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(path, bound_git_command(), args, None, 2 * 1024 * 1024, None).await
}

/// Keep credential-manager prompts suppressed for background fetches while
/// allowing the normal interactive flow for user-initiated operations.
pub(crate) async fn run_bound_remote_fetch(
    path: &Path,
    args: &[OsString],
    is_background_task: bool,
    report_progress: bool,
) -> Result<RawOutput> {
    let mut command = bound_git_command();
    // Never let an inherited progress path redirect Git LFS writes.
    command.env_remove("GIT_LFS_PROGRESS");
    command.env(
        "GCM_INTERACTIVE",
        if is_background_task { "0" } else { "1" },
    );
    let live = if report_progress {
        crate::hook_progress::sender().map(|(sender, request_id)| LiveOutput {
            sender,
            request_id,
            checkpoint: None,
            clone_progress: false,
            emit_clone_progress: false,
        })
    } else {
        None
    };
    let mut lfs = live
        .as_ref()
        .and_then(|_| crate::lfs_progress::ProgressFile::create().ok());
    if let Some(file) = &lfs {
        // `bound_git_command` has already applied the credential policy, so
        // only our private file, never an inherited path, reaches Git LFS.
        command.env("GIT_LFS_PROGRESS", file.path());
    }
    let work = run_command_inner(
        path,
        command,
        args,
        None,
        2 * 1024 * 1024,
        None,
        None,
        None,
        live.clone(),
    );
    tokio::pin!(work);
    let mut redactor = crate::stream_redaction::StreamRedactor::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let emit = |bytes: Vec<u8>| {
        if let Some(live) = &live {
            for chunk in bytes.chunks(8192) {
                let event = crate::hook_progress::CommitEvent::FetchOutput {
                    stream: "lfs",
                    data: STANDARD.encode(chunk),
                };
                let _ = live.sender.try_send((live.request_id, event));
            }
        }
    };
    let result = loop {
        tokio::select! {
            result = &mut work => break result,
            _ = tick.tick(), if lfs.is_some() => {
                if let Some(file) = &mut lfs {
                    let bytes = file.read_new().unwrap_or_default();
                    emit(redactor.push(&bytes, crate::git_credentials::live_output_secrets(false).await));
                }
            }
        }
    };
    if let Some(file) = &mut lfs {
        loop {
            let bytes = file.read_new().unwrap_or_default();
            if bytes.is_empty() {
                break;
            }
            emit(redactor.push(
                &bytes,
                crate::git_credentials::live_output_secrets(false).await,
            ));
        }
        emit(
            redactor.finish(
                crate::git_credentials::live_output_secrets(true)
                    .await
                    .unwrap_or_default(),
            ),
        );
    }
    result
}

/// Run a local tracking-branch fetch with bounded output and explicit stdin.
pub(crate) async fn run_bound_local_fetch(
    path: &Path,
    args: &[OsString],
    input: Vec<u8>,
) -> Result<RawOutput> {
    let mut command = bound_git_command();
    command.env("GIT_REFLOG_ACTION", "pull");
    run_command(path, command, args, Some(input), 2 * 1024 * 1024, None).await
}

/// Continue a Git operation without invoking an inherited interactive editor.
pub(crate) async fn run_bound_noninteractive(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    let mut command = bound_git_command();
    command.env("GIT_EDITOR", ":");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

/// Discover a repository without inherited command-scoped Git configuration.
pub(crate) async fn run_bound_discovery(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    let mut command = bound_git_command();
    command
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

/// Branch switches and their hooks use native path semantics. Fix diagnostic
/// locale so local-overwrite refusal is distinguishable from other failures.
pub async fn run_checkout(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    let mut command = git_native_paths(None);
    command.env("LC_ALL", "C");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

/// Run Git's conflict-marker diagnostic with a stable locale and an alternate index.
pub async fn conflict_markers(
    path: &Path,
    args: &[OsString],
    index: Option<&Path>,
) -> Result<RawOutput> {
    let mut command = git_command(index);
    command.env("LC_ALL", "C");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

/// LFS queries must not implicitly install hooks while inspecting attributes.
pub async fn run_lfs(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    // git-lfs uses Git's :(attr:filter=lfs) pathspec internally on newer Git.
    // Literal-path settings belong to our direct Git calls, not its children.
    let mut command = git_native_paths(None);
    command.env("GIT_LFS_TRACK_NO_INSTALL_HOOKS", "1");
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

/// Ask Git to prepare its global config and print the path passed to its editor.
pub async fn global_config_path(path: &Path) -> Result<RawOutput> {
    let mut command = git_native_paths(None);
    command.env("GIT_EDITOR", "printf %s");
    run_command(
        path,
        command,
        &crate::args(&["config", "--edit", "--global"]),
        None,
        64 * 1024,
        None,
    )
    .await
}

/// Run initialization checks without inheriting a different Git worktree.
pub(crate) async fn initialization_command(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    let mut command = git_native_paths(None);
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    run_command(path, command, args, None, 2 * 1024 * 1024, None).await
}

fn git_native_paths(index: Option<&Path>) -> Command {
    let mut command = git_base(index);
    command
        .env_remove("GIT_LITERAL_PATHSPECS")
        .env_remove("GIT_GLOB_PATHSPECS")
        .env_remove("GIT_NOGLOB_PATHSPECS")
        .env_remove("GIT_ICASE_PATHSPECS");
    command
}

fn git_command(index: Option<&Path>) -> Command {
    let mut command = git_base(index);
    command.arg("--literal-pathspecs");
    command
}

fn git_base(index: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command
        .args(["--no-pager", "-c", "color.ui=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GIT_OPTIONAL_LOCKS", "0");
    crate::git_credentials::apply(&mut command);
    // Keep user/credential policy out of persisted guard scripts. Native Git
    // appends command-line hook overrides to PARAMETERS after this snapshot.
    if let Some(parameters) = crate::git_credentials::inherited_parameters() {
        command
            .env("DESKTOP_RS_GIT_BASE_PARAMETERS_PRESENT", "1")
            .env("DESKTOP_RS_GIT_BASE_PARAMETERS", parameters);
    } else {
        command
            .env("DESKTOP_RS_GIT_BASE_PARAMETERS_PRESENT", "0")
            .env_remove("DESKTOP_RS_GIT_BASE_PARAMETERS");
    }
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
}

/// Stream checkout-filter output without imposing the UI preview limit.
pub async fn git_to_file(path: &Path, args: &[OsString], output: &Path) -> Result<RawOutput> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(output)?;
    run_command(
        path,
        git_command(None),
        args,
        None,
        0,
        Some(tokio::fs::File::from_std(file)),
    )
    .await
}

/// Stream an already-open regular file to Git without reopening its pathname.
pub(crate) async fn git_from_file(
    path: &Path,
    args: &[OsString],
    input: std::fs::File,
) -> Result<RawOutput> {
    run_command_inner(
        path,
        git_command(None),
        args,
        None,
        2 * 1024 * 1024,
        None,
        None,
        Some(input),
        None,
    )
    .await
}

pub async fn run_gh(path: &Path, args: &[OsString], input: Option<Vec<u8>>) -> Result<RawOutput> {
    let mut command = Command::new("gh");
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1");
    run_command(path, command, args, input, 2 * 1024 * 1024, None).await
}

/// Private API sessions capture credentials and responses without debug logging.
/// A supplied token is pinned in every gh credential environment variable; it
/// never appears in arguments or in this module's command diagnostics.
pub(crate) async fn run_gh_private(
    path: &Path,
    args: &[OsString],
    token: Option<&str>,
) -> Result<RawOutput> {
    run_gh_private_input(path, args, None, token).await
}

pub(crate) async fn run_gh_private_input(
    path: &Path,
    args: &[OsString],
    input: Option<Vec<u8>>,
    token: Option<&str>,
) -> Result<RawOutput> {
    run_command(
        path,
        private_gh_command(token),
        args,
        input,
        2 * 1024 * 1024,
        None,
    )
    .await
}

/// Inspect explicitly stored gh accounts without environment-token overrides.
pub(crate) async fn run_gh_stored(path: &Path, args: &[OsString]) -> Result<RawOutput> {
    run_command(path, stored_gh_command(), args, None, 2 * 1024 * 1024, None).await
}

fn stored_gh_command() -> Command {
    let mut command = private_gh_command(None);
    for variable in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
    ] {
        command.env_remove(variable);
    }
    command
}

fn private_gh_command(token: Option<&str>) -> Command {
    let mut command = Command::new("gh");
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env_remove("GH_DEBUG")
        .env_remove("DEBUG")
        .env_remove("GH_REPO")
        .env_remove("GH_HOST");
    crate::git_credentials::apply(&mut command);
    if let Some(token) = token {
        for variable in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            command.env(variable, token);
        }
    }
    command
}

/// Compare captured bytes with enough output budget for line-prefix overhead.
pub async fn diff_files(path: &Path, before: &Path, after: &Path) -> Result<RawOutput> {
    let mut command = Command::new("git");
    command.arg("--no-pager").env("GIT_OPTIONAL_LOCKS", "0");
    let mut args = crate::args(&[
        "diff",
        "--no-index",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
        "--",
    ]);
    args.extend([before.as_os_str().to_owned(), after.as_os_str().to_owned()]);
    run_command(path, command, &args, None, 8 * 1024 * 1024, None).await
}

async fn run_command(
    path: &Path,
    command: Command,
    args: &[OsString],
    input: Option<Vec<u8>>,
    output_limit: u64,
    output_file: Option<tokio::fs::File>,
) -> Result<RawOutput> {
    Box::pin(run_command_inner(
        path,
        command,
        args,
        input,
        output_limit,
        output_file,
        None,
        None,
        None,
    ))
    .await
}

#[derive(Clone)]
struct LiveOutput {
    sender: tokio::sync::mpsc::Sender<(u64, crate::hook_progress::CommitEvent)>,
    request_id: u64,
    checkpoint: Option<String>,
    clone_progress: bool,
    emit_clone_progress: bool,
}

#[derive(Clone, Copy)]
enum OutputSlice {
    Range(u64),
    PrefixAndDrain,
    BoundedBlob,
}

#[expect(
    clippy::too_many_arguments,
    reason = "Process I/O channels and limits have separate semantics"
)]
async fn run_command_inner(
    path: &Path,
    mut command: Command,
    args: &[OsString],
    input: Option<Vec<u8>>,
    output_limit: u64,
    output_file: Option<tokio::fs::File>,
    prefix_offset: Option<OutputSlice>,
    input_file: Option<std::fs::File>,
    live: Option<LiveOutput>,
) -> Result<RawOutput> {
    let cancellation = crate::cancellation::Cancellation::current();
    cancellation.check()?;
    #[cfg(unix)]
    command.process_group(0);
    let executable = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let mut child = command
        .args(args)
        .current_dir(path)
        .stdin(if let Some(file) = input_file {
            Stdio::from(file)
        } else if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!("start {executable}; verify it is installed and available on PATH")
        })?;
    let mut group = ProcessGroup::new(&child);
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().context("capture stdout")?;
    let stderr = child.stderr.take().context("capture stderr")?;
    let result = {
        let work = async {
            let input_task = async move {
                if let (Some(mut stdin), Some(input)) = (stdin, input) {
                    if let Err(error) = stdin.write_all(&input).await
                        && error.kind() != std::io::ErrorKind::BrokenPipe
                    {
                        return Err(error);
                    }
                    if let Err(error) = stdin.shutdown().await
                        && error.kind() != std::io::ErrorKind::BrokenPipe
                    {
                        return Err(error);
                    }
                }
                Ok::<(), std::io::Error>(())
            };
            let stdout_live = live.clone();
            let output = async move {
                if let Some(mut file) = output_file {
                    let mut stdout = stdout;
                    tokio::io::copy(&mut stdout, &mut file).await?;
                    file.sync_all().await?;
                    Ok(Vec::new())
                } else if let Some(slice) = prefix_offset {
                    let mut stdout = stdout;
                    if let OutputSlice::Range(offset) = slice {
                        let skipped = tokio::io::copy(
                            &mut (&mut stdout).take(offset),
                            &mut tokio::io::sink(),
                        )
                        .await?;
                        if skipped != offset {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "blob ended before requested offset",
                            ));
                        }
                    }
                    let mut bytes = Vec::new();
                    if matches!(slice, OutputSlice::BoundedBlob) {
                        return read_bounded(stdout, output_limit).await;
                    } else if matches!(slice, OutputSlice::PrefixAndDrain) {
                        let limit = output_limit as usize;
                        let mut buffer = vec![0_u8; 64 * 1024];
                        loop {
                            let count = stdout.read(&mut buffer).await?;
                            if count == 0 {
                                break;
                            }
                            let kept = count.min(limit - bytes.len());
                            if kept > 0 {
                                let needed = bytes.len() + kept;
                                if needed > bytes.capacity() {
                                    let step = if needed <= 1024 * 1024 {
                                        64 * 1024
                                    } else {
                                        1024 * 1024
                                    };
                                    let target = needed.next_multiple_of(step).min(limit);
                                    bytes.try_reserve_exact(target - bytes.len()).map_err(
                                        |_| {
                                            std::io::Error::other(
                                                "cannot allocate comparison snapshot",
                                            )
                                        },
                                    )?;
                                }
                                bytes.extend_from_slice(&buffer[..kept]);
                            }
                        }
                    } else {
                        (&mut stdout)
                            .take(output_limit)
                            .read_to_end(&mut bytes)
                            .await?;
                    }
                    Ok(bytes)
                } else {
                    Box::pin(read_output(stdout, output_limit, stdout_live, "stdout")).await
                }
            };
            let (stdout, stderr, status, ()) = tokio::try_join!(
                output,
                Box::pin(read_output(stderr, 2 * 1024 * 1024, live, "stderr")),
                child.wait(),
                input_task
            )?;
            Ok(RawOutput {
                code: status.code().unwrap_or(1),
                stdout,
                stderr,
            })
        };
        tokio::pin!(work);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                group.interrupt();
                let _ = tokio::time::timeout(Duration::from_secs(2), &mut work).await;
                Err(anyhow::anyhow!(cancellation.message()))
            },
            _ = tokio::time::sleep(Duration::from_secs(120)) => {
                group.interrupt();
                let _ = tokio::time::timeout(Duration::from_secs(2), &mut work).await;
                Err(anyhow::anyhow!(if cancellation.remote_mutation_started() {
                    "Command exceeded 120 seconds; inspect GitHub before retrying: the remote mutation may have completed"
                } else {
                    "Command exceeded 120 seconds; inspect repository state before retrying a mutation"
                }))
            },
            result = &mut work => result,
        }
    };
    if result.is_ok() {
        group.disarm();
    } else {
        group.terminate();
        // Wait for the direct child before releasing the repository lock.
        let _ = child.kill().await;
    }
    match result {
        // Private gh token capture must reach its bounded credential parser
        // unchanged. Session.run redacts published gh output itself; typed API
        // readers sanitize/whitelist private results. Git owns inherited hooks.
        Ok(mut output) if executable != "gh" => {
            // Blob bytes are repository data, not command diagnostics. Redacting
            // them would silently corrupt files that happen to contain a secret.
            let blob = prefix_offset.map(|_| std::mem::take(&mut output.stdout));
            let mut output = crate::git_credentials::redact(output).await;
            if let Some(blob) = blob {
                output.stdout = blob;
            }
            Ok(output)
        }
        Ok(output) => Ok(output),
        Err(error) => Err(error),
    }
}

/// A dropped future must also terminate hooks, credential helpers, and other descendants.
/// Unix commands have their own process group; Windows still uses direct-child cleanup.
pub(crate) struct ProcessGroup {
    #[cfg(unix)]
    pid: Option<i32>,
}
impl ProcessGroup {
    pub(crate) fn new(child: &tokio::process::Child) -> Self {
        #[cfg(not(unix))]
        let _ = child;
        Self {
            #[cfg(unix)]
            pid: child.id().and_then(|pid| i32::try_from(pid).ok()),
        }
    }
    pub(crate) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.pid = None;
        }
    }
    fn interrupt(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // SAFETY: this process group was created specifically for the command.
            unsafe {
                libc::kill(-pid, libc::SIGINT);
            }
        }
    }
    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.take() {
            // SAFETY: spawn used process_group(0); only this child's group is targeted.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

async fn read_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
    live: Option<LiveOutput>,
    stream: &'static str,
) -> std::io::Result<Vec<u8>> {
    let Some(live) = live else {
        return read_bounded(reader, limit).await;
    };
    if live.clone_progress {
        return read_clone_output(reader, limit, &live, stream).await;
    }
    let mut raw = Vec::new();
    let mut redactor = crate::stream_redaction::StreamRedactor::new();
    let mut buffer = Box::new([0; 8192]);
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    let mut sent = 0usize;
    let emit = |bytes: Vec<u8>, sent: &mut usize| {
        for chunk in bytes.chunks(8192) {
            if *sent >= 256 * 1024 {
                break;
            }
            *sent += chunk.len();
            let event = match &live.checkpoint {
                Some(checkpoint_id) => crate::hook_progress::CommitEvent::Output {
                    checkpoint_id: checkpoint_id.clone(),
                    stream,
                    data: STANDARD.encode(chunk),
                },
                None => crate::hook_progress::CommitEvent::FetchOutput {
                    stream,
                    data: STANDARD.encode(chunk),
                },
            };
            let _ = live.sender.try_send((live.request_id, event));
        }
    };
    loop {
        tokio::select! {
            read = reader.read(&mut buffer[..]) => {
                let count = read?;
                if count == 0 { break; }
                if raw.len().saturating_add(count) > limit as usize {
                    return Err(std::io::Error::other(format!("command output exceeds {limit} bytes; inspect state using the CLI")));
                }
                raw.extend_from_slice(&buffer[..count]);
                emit(redactor.push(&buffer[..count], crate::git_credentials::live_output_secrets(false).await), &mut sent);
            }
            _ = tick.tick() => emit(redactor.push(&[], crate::git_credentials::live_output_secrets(false).await), &mut sent),
        }
    }
    let secrets = crate::git_credentials::live_output_secrets(true)
        .await
        .unwrap_or_default();
    emit(redactor.finish(secrets), &mut sent);
    Ok(raw)
}

/// Clone diagnostics can contain rewritten URLs and credentials unknown to
/// Desktop. Forward only a fixed Git phase and a numeric percentage, never
/// raw child output or remote-controlled text.
async fn read_clone_output(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
    live: &LiveOutput,
    stream: &'static str,
) -> std::io::Result<Vec<u8>> {
    // Preserve both the beginning and end of the diagnostic stream. The
    // final fatal error is often after a large amount of progress output.
    // Always drain excess output so it cannot turn a successful clone into
    // a failed command or prevent Git from exiting.
    let maximum = limit as usize;
    let prefix_limit = maximum / 2;
    let tail_limit = maximum.saturating_sub(prefix_limit).saturating_sub(1);
    let mut prefix = Vec::new();
    let mut tail = std::collections::VecDeque::new();
    let mut truncated = false;
    let mut line = Vec::new();
    let mut discarded = false;
    // Keep at most six updates per phase, reserving a slot for 100%. This
    // bounds a whole clone to 48 messages, below the 64-message WS queue.
    let mut seen = [[false; 6]; 8];
    let mut buffer = Box::new([0_u8; 8192]);
    loop {
        let count = reader.read(&mut buffer[..]).await?;
        if count == 0 {
            break;
        }
        let head = count.min(prefix_limit.saturating_sub(prefix.len()));
        prefix.extend_from_slice(&buffer[..head]);
        tail.extend(buffer[head..count].iter().copied());
        if tail.len() > tail_limit {
            let excess = tail.len() - tail_limit;
            tail.drain(..excess);
            truncated = true;
        }
        if stream != "stderr" || !live.emit_clone_progress {
            continue;
        }
        for &byte in &buffer[..count] {
            if byte == b'\r' || byte == b'\n' {
                if !discarded
                    && let Some((phase, percent)) = clone_progress_line(&line)
                    && !seen[phase][(percent / 20) as usize]
                {
                    seen[phase][(percent / 20) as usize] = true;
                    let prefix = if CLONE_PROGRESS_PHASES[phase] == "Compressing objects" {
                        "remote: "
                    } else {
                        ""
                    };
                    let progress = format!(
                        "{prefix}{}: {percent}% ({percent}/100)\r",
                        CLONE_PROGRESS_PHASES[phase]
                    );
                    let event = crate::hook_progress::CommitEvent::FetchOutput {
                        stream: "stderr",
                        data: STANDARD.encode(progress),
                    };
                    let _ = live.sender.try_send((live.request_id, event));
                }
                line.clear();
                discarded = false;
            } else if line.len() < 1024 && !discarded {
                line.push(byte);
            } else {
                line.clear();
                discarded = true;
            }
        }
    }
    if truncated && maximum > 0 {
        // Keep unrelated head/tail fragments from forming a synthetic line.
        prefix.push(b'\n');
    }
    prefix.extend(tail);
    Ok(prefix)
}

const CLONE_PROGRESS_PHASES: [&str; 8] = [
    "Enumerating objects",
    "Counting objects",
    "Compressing objects",
    "Receiving objects",
    "Resolving deltas",
    "Checking out files",
    "Updating files",
    "Filtering content",
];

fn clone_progress_line(line: &[u8]) -> Option<(usize, u8)> {
    let line = line.strip_prefix(b"remote: ").unwrap_or(line);
    for (index, phase) in CLONE_PROGRESS_PHASES.iter().enumerate() {
        let Some(rest) = line
            .strip_prefix(phase.as_bytes())
            .and_then(|s| s.strip_prefix(b":"))
        else {
            continue;
        };
        let rest = rest
            .strip_prefix(b" ")
            .map_or(rest, |_| rest.trim_ascii_start());
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=3).contains(&digits) || rest.get(digits) != Some(&b'%') {
            return None;
        }
        let percent = std::str::from_utf8(&rest[..digits])
            .ok()?
            .parse::<u8>()
            .ok()?;
        if percent > 100 {
            return None;
        }
        return Some((index, percent));
    }
    None
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() > limit as usize {
        return Err(std::io::Error::other(format!(
            "command output exceeds {limit} bytes; inspect state using the CLI"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn clone_inherits_child_local_directory_fd_without_persisting_it() {
        use std::os::fd::AsRawFd;
        let root = std::env::temp_dir().join(format!("desktop-child-fd-{}", uuid::Uuid::new_v4()));
        let source = root.join("source.git");
        let destination = root.join("destination");
        std::fs::create_dir_all(&destination).unwrap();
        let init = std::process::Command::new("git").args(["init", "--bare", "--quiet"]).arg(&source).status().unwrap();
        assert!(init.success());
        let handle = std::fs::File::open(&destination).unwrap();
        let fd = handle.as_raw_fd();
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
        let args = vec!["clone".into(), "--no-checkout".into(), "--".into(), source.as_os_str().to_owned(), format!("/proc/self/fd/{fd}").into()];
        let result = run_clone_with_inherited_fd(&root, &args, false, &handle, None).await.unwrap();
        assert_eq!(result.code, 0, "{}", String::from_utf8_lossy(&result.stderr));
        assert!(destination.join(".git").is_dir());
        assert!(!std::fs::read_to_string(destination.join(".git/config")).unwrap().contains("/proc/self/fd"));
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn clone_rejects_directory_fd_overlapping_standard_stream() {
        use std::os::fd::{AsRawFd, FromRawFd};
        const CHILD: &str = "DESKTOP_TEST_CLONE_CLOSED_STDIN";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process::tests::clone_rejects_directory_fd_overlapping_standard_stream", "--nocapture"])
                .env(CHILD, "1").status().unwrap();
            assert!(status.success());
            return;
        }
        let directory = std::fs::File::open(std::env::temp_dir()).unwrap();
        assert_eq!(unsafe { libc::dup2(directory.as_raw_fd(), 0) }, 0);
        let handle = unsafe { std::fs::File::from_raw_fd(0) };
        let error = run_clone_with_inherited_fd(Path::new("/"), &[], false, &handle, None).await.err().unwrap();
        assert!(error.to_string().contains("standard stream"));
    }

    #[tokio::test]
    async fn clone_progress_never_forwards_split_proxy_credentials() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let live = LiveOutput {
            sender,
            request_id: 72,
            checkpoint: None,
            clone_progress: true,
            emit_clone_progress: true,
        };
        let (mut writer, reader) = tokio::io::duplex(8);
        let producer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for piece in [
                "Receiving objects: 25% https://user:pa",
                "ss%40word@proxy.invalid\r",
                "remote: Counting objects: 100% Basic dXNlcjpwYXNz\n",
            ] {
                writer.write_all(piece.as_bytes()).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let raw = read_clone_output(reader, 1024, &live, "stderr")
            .await
            .unwrap();
        producer.await.unwrap();
        assert!(String::from_utf8(raw).unwrap().contains("pass%40word"));
        let mut progress = Vec::new();
        while let Ok((_, event)) = receiver.try_recv() {
            if let crate::hook_progress::CommitEvent::FetchOutput { data, .. } = event {
                progress.push(String::from_utf8(STANDARD.decode(data).unwrap()).unwrap());
            }
        }
        assert_eq!(
            progress,
            [
                "Receiving objects: 25% (25/100)\r",
                "Counting objects: 100% (100/100)\r"
            ]
        );
    }

    #[tokio::test]
    async fn clone_progress_filters_secrets_and_duplicate_bursts() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
        let live = LiveOutput {
            sender,
            request_id: 71,
            checkpoint: None,
            clone_progress: true,
            emit_clone_progress: true,
        };
        let mut raw =
            b"fatal: https://user:SECRET@example.invalid/\nremote: Receiving objects: 25% SECRET\r"
                .to_vec();
        for _ in 0..500 {
            raw.extend_from_slice(b"Receiving objects: 25% token=SECRET\r");
        }
        for percent in 0..100 {
            raw.extend_from_slice(format!("Receiving objects: {percent}% SECRET\r").as_bytes());
        }
        raw.extend_from_slice(b"Receiving objects: 100% SECRET\rResolving deltas: 100% SECRET\rCounting objects: 101%\r");
        raw.extend_from_slice(&vec![b'x'; 2048]);
        raw.extend_from_slice(b"Receiving objects: 90% secret\r");
        let captured = read_clone_output(raw.as_slice(), 2 * 1024 * 1024, &live, "stderr")
            .await
            .unwrap();
        assert_eq!(captured, raw);
        let mut progress = Vec::new();
        while let Ok((id, event)) = receiver.try_recv() {
            assert_eq!(id, 71);
            if let crate::hook_progress::CommitEvent::FetchOutput { stream, data } = event {
                assert_eq!(stream, "stderr");
                progress.push(String::from_utf8(STANDARD.decode(data).unwrap()).unwrap());
            }
        }
        assert_eq!(
            progress,
            [
                "Receiving objects: 25% (25/100)\r",
                "Receiving objects: 0% (0/100)\r",
                "Receiving objects: 40% (40/100)\r",
                "Receiving objects: 60% (60/100)\r",
                "Receiving objects: 80% (80/100)\r",
                "Receiving objects: 100% (100/100)\r",
                "Resolving deltas: 100% (100/100)\r"
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_progress_drains_excess_output_and_keeps_failure_tail() {
        for (code, report_progress) in [(0, true), (9, true), (0, false), (9, false)] {
            let suffix = if code == 0 {
                "completed"
            } else {
                "fatal: Authentication failed for 'https://user:SECRET@example.invalid/repo'"
            };
            let shell = format!(
                "alias.probe=!printf 'Receiving objects: 25%% (1/4)\\r' >&2; head -c 2200000 /dev/zero | tr '\\000' x; head -c 2200000 /dev/zero | tr '\\000' y >&2; printf '\\n%s\\n' \"{suffix}\" >&2; exit {code}"
            );
            let command = crate::args(&["-c", &shell, "probe"]);
            let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
            let result = crate::hook_progress::scope(
                sender,
                73,
                run_clone(&std::env::temp_dir(), &command, report_progress),
            )
            .await
            .unwrap();
            assert_eq!(result.code, code);
            assert!(result.stdout.len() <= 2 * 1024 * 1024);
            assert!(result.stderr.len() <= 2 * 1024 * 1024);
            assert!(result.stderr.ends_with(format!("{suffix}\n").as_bytes()));
            if code != 0 {
                assert_eq!(
                    crate::fetch_error::classify(&result.stderr, &result.stdout, None),
                    Some("https_authentication_failed")
                );
            }
            while let Ok((_, event)) = receiver.try_recv() {
                if let crate::hook_progress::CommitEvent::FetchOutput { data, .. } = event {
                    assert!(
                        !STANDARD
                            .decode(data)
                            .unwrap()
                            .windows(6)
                            .any(|w| w == b"SECRET")
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clone_runner_cancellation_stops_the_child_process_group() {
        let root =
            std::env::temp_dir().join(format!("desktop-clone-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let started = root.join("started");
        let after = root.join("after");
        let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let command = crate::args(&[
            "-c",
            &format!(
                "alias.probe=!printf x > {}; sleep 3; printf x > {}",
                quote(&started),
                quote(&after)
            ),
            "probe",
        ]);
        let cancellation = crate::cancellation::Cancellation::default();
        let task = tokio::spawn(
            cancellation
                .clone()
                .scope(async move { run_clone(&root, &command, true).await }),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(!after.exists());
        std::fs::remove_dir_all(started.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_fetch_sets_credential_manager_interactivity_for_descendants() {
        let command = crate::args(&["-c", "alias.probe=!printf %s \"$GCM_INTERACTIVE\"", "probe"]);
        for (background, expected) in [(true, &b"0"[..]), (false, &b"1"[..])] {
            let result = run_bound_remote_fetch(&std::env::temp_dir(), &command, background, false)
                .await
                .unwrap();
            assert_eq!(result.code, 0);
            assert_eq!(result.stdout, expected);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_fetch_ignores_inherited_lfs_progress_path() {
        const PROBE: &str = "DESKTOP_TEST_LFS_PROGRESS_INHERITANCE_CHILD";
        if std::env::var_os(PROBE).is_none() {
            // A separate test process avoids modifying the environment of
            // concurrently running tests.
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process::tests::remote_fetch_ignores_inherited_lfs_progress_path",
                ])
                .env(PROBE, "1")
                .env("GIT_LFS_PROGRESS", "/untrusted/progress")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            return;
        }
        let command = crate::args(&[
            "-c",
            "alias.probe=!printf %s \"${GIT_LFS_PROGRESS-unset}\"",
            "probe",
        ]);
        let result = run_bound_remote_fetch(&std::env::temp_dir(), &command, false, false)
            .await
            .unwrap();
        assert_eq!(result.code, 0);
        assert_eq!(result.stdout, b"unset");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_fetch_streams_request_owned_git_and_lfs_output() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
        let command = crate::args(&[
            "-c",
            "alias.probe=!printf 'Receiving objects: 50%% (1/2)\\r' >&2; printf 'download 1/2 4/8 example\\n' >> \"$GIT_LFS_PROGRESS\"",
            "probe",
        ]);
        let output = crate::hook_progress::scope(
            sender,
            42,
            run_bound_remote_fetch(&std::env::temp_dir(), &command, false, true),
        )
        .await
        .unwrap();
        assert_eq!(output.code, 0);
        let mut git = Vec::new();
        let mut lfs = Vec::new();
        while let Ok((request, event)) = receiver.try_recv() {
            assert_eq!(request, 42);
            if let crate::hook_progress::CommitEvent::FetchOutput { stream, data } = event {
                let bytes = STANDARD.decode(data).unwrap();
                match stream {
                    "stderr" => git.extend(bytes),
                    "lfs" => lfs.extend(bytes),
                    _ => {}
                }
            }
        }
        assert!(String::from_utf8_lossy(&git).contains("Receiving objects"));
        assert!(String::from_utf8_lossy(&lfs).contains("download 1/2"));
    }

    #[test]
    fn bad_revision_classification_does_not_hide_other_git_failures() {
        let output = |code, stderr: &[u8]| RawOutput {
            code,
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
        };
        assert!(output(128, b"fatal: bad revision 'missing..HEAD'\n").bad_revision());
        assert!(!output(0, b"fatal: bad revision 'missing'\n").bad_revision());
        assert!(!output(128, b"fatal: not a git repository\n").bad_revision());
        assert!(!output(128, b"fatal: bad revision 'unterminated\n").bad_revision());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "streams 70 MB from a Git alias"]
    async fn oversized_comparison_capture_preserves_git_failure() {
        let command = crate::args(&[
            "-c",
            "alias.probe=!head -c 70000002 /dev/zero; echo failed >&2; exit 9",
            "probe",
        ]);
        let output = run_comparison_snapshot(&std::env::temp_dir(), &command)
            .await
            .unwrap();
        assert_eq!(
            output.stdout.len(),
            crate::comparison_snapshots::MAX_BYTES + 1
        );
        assert_eq!(output.code, 9);
        assert!(String::from_utf8_lossy(&output.stderr).contains("failed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn small_comparison_capture_does_not_reserve_the_full_limit() {
        let command = crate::args(&["-c", "alias.probe=!printf small", "probe"]);
        let output = run_comparison_snapshot(&std::env::temp_dir(), &command)
            .await
            .unwrap();
        assert_eq!(output.stdout, b"small");
        assert!(output.stdout.capacity() <= 64 * 1024);
    }
    #[test]
    fn private_api_commands_pin_credentials_without_logging_or_repository_defaults() {
        let command = private_gh_command(Some("synthetic-session-token"));
        let environment: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
        for variable in ["GH_DEBUG", "DEBUG", "GH_REPO", "GH_HOST"] {
            assert_eq!(environment.get(std::ffi::OsStr::new(variable)), Some(&None));
        }
        for variable in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            assert_eq!(
                environment.get(std::ffi::OsStr::new(variable)),
                Some(&Some(std::ffi::OsStr::new("synthetic-session-token")))
            );
        }
        assert_eq!(command.as_std().get_args().count(), 0);
        let acquisition = private_gh_command(None);
        let environment: std::collections::BTreeMap<_, _> =
            acquisition.as_std().get_envs().collect();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("GH_DEBUG")),
            Some(&None)
        );
        assert_eq!(environment.get(std::ffi::OsStr::new("DEBUG")), Some(&None));
        assert!(!environment.contains_key(std::ffi::OsStr::new("GH_TOKEN")));
    }

    #[test]
    fn stored_account_commands_ignore_every_environment_token() {
        let command = stored_gh_command();
        let environment: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
        for variable in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_DEBUG",
            "DEBUG",
            "GH_REPO",
            "GH_HOST",
        ] {
            assert_eq!(environment.get(std::ffi::OsStr::new(variable)), Some(&None));
        }
        assert_eq!(command.as_std().get_args().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installed_gh_named_user_never_falls_back_to_conflicting_environment_tokens() {
        use std::{
            io::Write,
            os::unix::fs::{OpenOptionsExt, PermissionsExt},
        };
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("desktop-gh-named-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _cleanup = Cleanup(directory.clone());
        let mut probe = stored_gh_command();
        probe.arg("--version").env("GH_CONFIG_DIR", &directory);
        match probe.output().await {
            Ok(output) if output.status.success() => {}
            _ => {
                eprintln!("SKIP: installed gh named-user contract requires gh on PATH");
                return;
            }
        }
        // Unique user names ensure this fixture can never select an existing
        // real keychain entry, even on a developer's logged-in machine.
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let active = format!("ActiveFixture-{nonce}");
        let chosen = format!("ChosenFixture-{nonce}");
        let missing = format!("MissingFixture-{nonce}");
        let mut configuration = String::new();
        for host in ["github.com", "account-fixture.invalid"] {
            configuration.push_str(&format!("{host}:\n    user: {active}\n    oauth_token: synthetic-active\n    git_protocol: https\n    users:\n        {active}:\n            oauth_token: synthetic-active\n        {chosen}:\n            oauth_token: synthetic-named\n"));
        }
        let path = directory.join("hosts.yml");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(configuration.as_bytes()).unwrap();
        drop(file);
        for host in ["github.com", "account-fixture.invalid"] {
            for (user, available) in [
                (chosen.clone(), true),
                (chosen.to_ascii_lowercase(), false),
                (missing.clone(), false),
            ] {
                let mut command = stored_gh_command();
                command.env("GH_CONFIG_DIR", &directory);
                for variable in [
                    "GH_TOKEN",
                    "GITHUB_TOKEN",
                    "GH_ENTERPRISE_TOKEN",
                    "GITHUB_ENTERPRISE_TOKEN",
                ] {
                    command.env(variable, "synthetic-conflicting-environment");
                }
                let output = run_command(
                    &directory,
                    command,
                    &crate::args(&[
                        "auth",
                        "token",
                        "--hostname",
                        host,
                        "--user",
                        &user,
                        "--no-refresh",
                    ]),
                    None,
                    64 * 1024,
                    None,
                )
                .await
                .unwrap();
                if available {
                    assert_eq!(output.code, 0, "named stored-account lookup failed");
                    assert!(
                        output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout)
                            == b"synthetic-named",
                        "gh did not select the explicitly named synthetic account"
                    );
                } else {
                    assert_eq!(
                        output.code, 1,
                        "missing named-account lookup unexpectedly succeeded"
                    );
                    assert!(
                        output.stdout.is_empty(),
                        "missing named-account lookup emitted a credential"
                    );
                }
            }
        }
        assert!(
            std::fs::read(&path).unwrap() == configuration.as_bytes(),
            "read-only token lookup rewrote the isolated gh configuration"
        );
    }

    #[tokio::test]
    async fn output_limit_boundary() {
        let bytes = vec![b'x'; 2 * 1024 * 1024 + 1];
        assert!(
            read_bounded(bytes.as_slice(), 2 * 1024 * 1024)
                .await
                .is_err()
        );
        assert_eq!(
            read_bounded(&bytes[..bytes.len() - 1], 2 * 1024 * 1024)
                .await
                .unwrap()
                .len(),
            bytes.len() - 1
        );
    }
}
