//! Per-push hook proxy. Git remains alive while the owning UI decides whether
//! to ignore one failed pre-push hook; no push is retried.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use tokio::io::unix::AsyncFd;

const CAPTURE_LIMIT: u64 = 24 * 1024;
const MAX_ENTRIES: usize = 4096;

fn quote(value: &[u8]) -> Vec<u8> {
    let mut quoted = vec![b'\''];
    for byte in value {
        if *byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(*byte);
        }
    }
    quoted.push(b'\'');
    quoted
}

fn fifo(path: &Path) -> Result<()> {
    let name = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: name is a NUL-terminated path in our newly created private directory.
    if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error()).context("cannot create hook decision pipe");
    }
    Ok(())
}

fn pipe(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?)
}

fn find_utility(name: &str) -> Result<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(name);
        if candidate.is_absolute()
            && fs::metadata(&candidate).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
        {
            return Ok(candidate);
        }
    }
    anyhow::bail!("interactive pre-push requires the {name} utility")
}

pub(crate) struct HookProxy {
    directory: PathBuf,
    request: AsyncFd<File>,
    response: File,
    capture: PathBuf,
    sender: tokio::sync::mpsc::Sender<(u64, crate::hook_progress::CommitEvent)>,
    request_id: u64,
}
impl HookProxy {
    pub(crate) fn create(hooks: &Path, shell: &[u8]) -> Result<Option<Self>> {
        ensure!(
            shell.starts_with(b"/") && !shell.iter().any(u8::is_ascii_whitespace),
            "unsupported Git shell"
        );
        let pre_push = hooks.join("pre-push");
        let metadata = match fs::metadata(&pre_push) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
            return Ok(None);
        }
        let (sender, request_id) = crate::hook_progress::sender()
            .context("interactive hooks require a request-owned progress channel")?;
        let tee = find_utility("tee")?;
        let dd = find_utility("dd")?;
        let cat = find_utility("cat")?;
        let directory =
            std::env::temp_dir().join(format!("desktop-push-hooks-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(&directory)?;
        let result = (|| -> Result<Self> {
            let request_path = directory.join("decision-request");
            let response_path = directory.join("decision-response");
            let capture = directory.join("hook-output");
            let status = directory.join("hook-status");
            fifo(&request_path)?;
            fifo(&response_path)?;
            // SAFETY: File owns this stable descriptor for the AsyncFd lifetime.
            let request = unsafe {
                AsyncFd::register_with_interest(pipe(&request_path)?, tokio::io::Interest::READABLE)
            }?;
            let response = pipe(&response_path)?;
            let mut count = 0usize;
            let mut pre_push_wrapped = false;
            for entry in fs::read_dir(hooks)? {
                count += 1;
                ensure!(
                    count <= MAX_ENTRIES,
                    "too many files in the hooks directory"
                );
                let entry = entry?;
                let source = entry.path();
                let metadata = match fs::metadata(&source) {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
                    continue;
                }
                let name: OsString = entry.file_name();
                let mut script = b"#!".to_vec();
                script.extend_from_slice(shell);
                script.extend_from_slice(b"\nif [ \"${DESKTOP_RS_GIT_BASE_PARAMETERS_PRESENT:-0}\" = 1 ]; then\n GIT_CONFIG_PARAMETERS=$DESKTOP_RS_GIT_BASE_PARAMETERS\nelse\n GIT_CONFIG_PARAMETERS=\nfi\nGIT_CONFIG_PARAMETERS=\"${GIT_CONFIG_PARAMETERS:+$GIT_CONFIG_PARAMETERS }'color.ui'='false'\"\nexport GIT_CONFIG_PARAMETERS\n");
                if name == "pre-push" {
                    pre_push_wrapped = true;
                    script.extend_from_slice(b"( ");
                    script.extend(quote(source.as_os_str().as_bytes()));
                    script.extend_from_slice(b" \"$@\"; printf '%s\\n' \"$?\" > ");
                    script.extend(quote(status.as_os_str().as_bytes()));
                    // Save only a bounded prefix, then keep draining the pipe
                    // so a verbose hook is never killed by SIGPIPE.
                    script.extend_from_slice(b" ) 2>&1 | ");
                    script.extend(quote(tee.as_os_str().as_bytes()));
                    script.extend_from_slice(b" /dev/stderr | { ");
                    script.extend(quote(dd.as_os_str().as_bytes()));
                    script.extend_from_slice(b" bs=1024 count=1024 of=");
                    script.extend(quote(capture.as_os_str().as_bytes()));
                    script.extend_from_slice(b" 2>/dev/null; capture_status=$?; ");
                    script.extend(quote(cat.as_os_str().as_bytes()));
                    script.extend_from_slice(b" >/dev/null; drain_status=$?; [ \"$capture_status\" -eq 0 ] && [ \"$drain_status\" -eq 0 ]; }\npipe_status=$?\nIFS= read -r status < ");
                    script.extend(quote(status.as_os_str().as_bytes()));
                    script.extend_from_slice(b" || exit 1\n[ \"$pipe_status\" -eq 0 ] || exit 1\n[ \"$status\" -eq 0 ] && exit 0\nprintf 'failed\\n' > ");
                    script.extend(quote(request_path.as_os_str().as_bytes()));
                    script.extend_from_slice(b" || exit \"$status\"\nIFS= read -r decision < ");
                    script.extend(quote(response_path.as_os_str().as_bytes()));
                    script.extend_from_slice(b" || exit \"$status\"\n[ \"$decision\" = ignore ] && exit 0\nexit \"$status\"\n");
                } else {
                    let quoted = quote(source.as_os_str().as_bytes());
                    script.extend_from_slice(b"if [ -f ");
                    script.extend(&quoted);
                    script.extend_from_slice(b" ] && [ -x ");
                    script.extend(&quoted);
                    script.extend_from_slice(b" ]; then exec ");
                    script.extend(&quoted);
                    script.extend_from_slice(b" \"$@\"; fi\nexit 0\n");
                }
                let destination = directory.join(name);
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o700)
                    .open(destination)?;
                file.write_all(&script)?;
                file.set_permissions(fs::Permissions::from_mode(0o700))?;
            }
            ensure!(pre_push_wrapped, "pre-push hook changed during proxy setup");
            // A hook may be installed while Git waits for the decision. Proxy
            // the standard hook names too, checking the original path at call
            // time. Server-side hooks are resolved in the remote repository.
            for name in [
                "applypatch-msg",
                "pre-applypatch",
                "post-applypatch",
                "pre-commit",
                "pre-merge-commit",
                "prepare-commit-msg",
                "commit-msg",
                "post-commit",
                "pre-rebase",
                "post-checkout",
                "post-merge",
                "pre-receive",
                "update",
                "proc-receive",
                "post-receive",
                "post-update",
                "reference-transaction",
                "push-to-checkout",
                "pre-auto-gc",
                "post-rewrite",
                "sendemail-validate",
                "fsmonitor-watchman",
                "p4-changelist",
                "p4-prepare-changelist",
                "p4-post-changelist",
                "p4-pre-submit",
                "post-index-change",
            ] {
                let destination = directory.join(name);
                if destination.exists() {
                    continue;
                }
                let source = hooks.join(name);
                let quoted = quote(source.as_os_str().as_bytes());
                let mut script = b"#!".to_vec();
                script.extend_from_slice(shell);
                script.extend_from_slice(b"\nif [ \"${DESKTOP_RS_GIT_BASE_PARAMETERS_PRESENT:-0}\" = 1 ]; then\n GIT_CONFIG_PARAMETERS=$DESKTOP_RS_GIT_BASE_PARAMETERS\nelse\n GIT_CONFIG_PARAMETERS=\nfi\nGIT_CONFIG_PARAMETERS=\"${GIT_CONFIG_PARAMETERS:+$GIT_CONFIG_PARAMETERS }'color.ui'='false'\"\nexport GIT_CONFIG_PARAMETERS\nif [ -f ");
                script.extend(&quoted);
                script.extend_from_slice(b" ] && [ -x ");
                script.extend(&quoted);
                script.extend_from_slice(b" ]; then exec ");
                script.extend(&quoted);
                script.extend_from_slice(b" \"$@\"; fi\nexit 0\n");
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o700)
                    .open(destination)?;
                file.write_all(&script)?;
            }
            Ok(Self {
                directory: directory.clone(),
                request,
                response,
                capture,
                sender,
                request_id,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&directory);
        }
        result.map(Some)
    }
    pub(crate) fn configuration(&self) -> Vec<OsString> {
        let mut setting = OsString::from("core.hooksPath=");
        setting.push(&self.directory);
        vec!["-c".into(), setting]
    }
    pub(crate) async fn decide(&mut self) -> Result<()> {
        let mut bytes = [0u8; 32];
        let mut request = Vec::new();
        loop {
            let mut ready = self.request.readable().await?;
            match ready.try_io(|fd| fd.get_ref().read(&mut bytes)) {
                Ok(Ok(0)) | Err(_) => continue,
                Ok(Ok(size)) => {
                    request.extend_from_slice(&bytes[..size]);
                    ensure!(
                        b"failed\n".starts_with(&request),
                        "invalid hook decision request"
                    );
                    if request == b"failed\n" {
                        break;
                    }
                }
                Ok(Err(error)) => return Err(error.into()),
            }
        }
        let file = File::open(&self.capture)?;
        ensure!(
            file.metadata()?.len() <= 2 * 1024 * 1024,
            "hook output exceeds limit"
        );
        let mut bytes = Vec::new();
        file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 2 * 1024 * 1024, "hook output exceeds limit");
        let secrets = crate::git_credentials::live_output_secrets(true)
            .await
            .context("cannot safely redact hook output")?;
        let mut redactor = crate::stream_redaction::StreamRedactor::new();
        let mut safe = redactor.push(&bytes, Some(secrets.clone()));
        safe.extend(redactor.finish(secrets));
        if safe.len() > CAPTURE_LIMIT as usize {
            safe.drain(..safe.len() - CAPTURE_LIMIT as usize);
        }
        let mut decision = crate::hook_decision::PendingDecision::new()?;
        let event = crate::hook_progress::CommitEvent::HookDecision {
            decision_id: decision.id(),
            output: STANDARD.encode(safe),
        };
        let cancellation = crate::cancellation::Cancellation::current();
        let sent = tokio::select! {
            biased;
            _ = cancellation.cancelled() => false,
            value = self.sender.send((self.request_id, event)) => value.is_ok(),
        };
        let ignore = sent && decision.wait().await;
        self.response
            .write_all(if ignore { b"ignore\n" } else { b"abort\n" })?;
        Ok(())
    }
}
impl Drop for HookProxy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
