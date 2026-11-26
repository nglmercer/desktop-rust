//! Prepared ref transactions whose verification decision belongs to one request.
//!
//! Git holds the real ref and HEAD locks while the renderer verifies state. The
//! verification capability is single-use and fails closed on cancellation,
//! timeout, process death, or transport loss.
use crate::{Repository, hook_decision::PendingDecision, hook_progress::CommitEvent, process};
use anyhow::{Context, Result, ensure};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_reference(value: &str) -> bool {
    if value == "HEAD" {
        return true;
    }
    if !value.starts_with("refs/") || value.len() > 4096 {
        return false;
    }
    value.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && !part.ends_with('.')
            && !part.ends_with(".lock")
            && !part.contains("..")
            && part.bytes().all(|byte| {
                byte > 0x20
                    && byte != 0x7f
                    && !matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
            })
    })
}

fn validate(
    reference: &str,
    old_value: &str,
    new_value: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    ensure!(valid_reference(reference), "Invalid guarded ref");
    ensure!(valid_oid(old_value), "Invalid guarded old object ID");
    if let Some(value) = new_value {
        ensure!(valid_oid(value), "Invalid guarded new object ID");
    }
    if let Some(reason) = reason {
        ensure!(
            !reason.is_empty() && reason.len() <= 4096 && !reason.chars().any(char::is_control),
            "Invalid reflog reason"
        );
    }
    Ok(())
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other(
            "ref transaction output exceeded its limit",
        ));
    }
    Ok(bytes)
}

impl Repository {
    /// Run `git update-ref --stdin -z` through its prepared state and wait for
    /// the owning renderer request to approve exactly one commit decision.
    pub(crate) async fn renderer_update_ref_transaction(
        &self,
        reference: &str,
        old_value: &str,
        new_value: Option<&str>,
        reason: Option<&str>,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<()> {
        validate(reference, old_value, new_value, reason)?;
        for key in environment.keys() {
            anyhow::ensure!(
                crate::assisted_head::assisted_push_config_environment_key(key),
                "unsupported ref transaction environment key"
            );
        }
        let mut command = process::git_command(None);
        process::apply_environment(&mut command, environment)?;
        command.arg("update-ref");
        if let Some(reason) = reason {
            command.arg("-m").arg(reason);
        }
        command.args(["--stdin", "-z"]);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .current_dir(self.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("start git update-ref; verify it is installed and available on PATH")?;
        let mut group = process::ProcessGroup::new(&child);
        let mut stdin = child
            .stdin
            .take()
            .context("capture ref transaction stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("capture ref transaction stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("capture ref transaction stderr")?;
        let mut stderr_task = tokio::spawn(read_bounded(stderr, 256 * 1024));
        let cancellation = crate::cancellation::Cancellation::current();
        let result = {
            let work = async {
                let mut input = Vec::with_capacity(4096);
                input.extend_from_slice(b"start\0option no-deref\0");
                match new_value {
                    Some(new_value) => {
                        input.extend_from_slice(b"update ");
                        input.extend_from_slice(reference.as_bytes());
                        input.push(0);
                        input.extend_from_slice(new_value.as_bytes());
                        input.push(0);
                        input.extend_from_slice(old_value.as_bytes());
                        input.push(0);
                    }
                    None => {
                        input.extend_from_slice(b"delete ");
                        input.extend_from_slice(reference.as_bytes());
                        input.push(0);
                        input.extend_from_slice(old_value.as_bytes());
                        input.push(0);
                    }
                }
                input.extend_from_slice(b"prepare\0");
                stdin
                    .write_all(&input)
                    .await
                    .context("write ref transaction")?;

                let mut stdout = BufReader::new(stdout);
                let mut prepared = false;
                for _ in 0..16 {
                    let mut line = Vec::new();
                    let count = stdout
                        .read_until(b'\n', &mut line)
                        .await
                        .context("read ref transaction preparation")?;
                    ensure!(count > 0, "Git ref transaction did not prepare");
                    if line == b"prepare: ok\n" || line == b"prepare: ok\r\n" {
                        prepared = true;
                        break;
                    }
                    ensure!(
                        !line.starts_with(b"prepare: error"),
                        "Git ref transaction did not prepare"
                    );
                }
                ensure!(prepared, "Git ref transaction did not prepare");

                let (sender, request_id) = crate::hook_progress::sender()
                    .context("Ref verification requires an owning backend request")?;
                let mut decision = PendingDecision::new()?;
                let sent = sender
                    .send((
                        request_id,
                        CommitEvent::HookDecision {
                            decision_id: decision.id(),
                            output: String::new(),
                        },
                    ))
                    .await
                    .is_ok();
                let approved = sent && decision.wait().await;
                stdin
                    .write_all(if approved { b"commit\0" } else { b"abort\0" })
                    .await
                    .context("finish ref transaction")?;
                stdin
                    .shutdown()
                    .await
                    .context("close ref transaction input")?;
                drop(stdin);
                let status = child.wait().await.context("wait for ref transaction")?;
                let _stderr = (&mut stderr_task)
                    .await
                    .context("join ref transaction diagnostics")?
                    .context("read ref transaction diagnostics")?;
                ensure!(approved, "Ref transaction verification was rejected");
                ensure!(status.success(), "Git ref transaction failed");
                Ok::<(), anyhow::Error>(())
            };
            tokio::pin!(work);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    group.interrupt();
                    let _ = tokio::time::timeout(Duration::from_secs(2), &mut work).await;
                    Err(anyhow::anyhow!(cancellation.message()))
                }
                _ = tokio::time::sleep(Duration::from_secs(120)) => {
                    group.interrupt();
                    let _ = tokio::time::timeout(Duration::from_secs(2), &mut work).await;
                    Err(anyhow::anyhow!("Ref transaction timed out; inspect repository refs before retrying"))
                }
                result = &mut work => result,
            }
        };
        if result.is_ok() {
            group.disarm();
        } else {
            group.terminate();
            let _ = child.kill().await;
            stderr_task.abort();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_validation_is_conservative() {
        assert!(valid_reference("HEAD"));
        assert!(valid_reference("refs/heads/main"));
        assert!(!valid_reference("main"));
        assert!(!valid_reference("refs/heads/../main"));
        assert!(!valid_reference("refs/heads/bad lock"));
        assert!(!valid_reference("refs/heads/bad.lock"));
    }

    #[test]
    fn oid_validation_accepts_sha1_and_sha256() {
        assert!(valid_oid(&"0".repeat(40)));
        assert!(valid_oid(&"a".repeat(64)));
        assert!(!valid_oid(&"g".repeat(40)));
        assert!(!valid_oid(&"0".repeat(39)));
    }
}
