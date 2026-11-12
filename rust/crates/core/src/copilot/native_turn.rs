//! Experimental one-turn client for the pinned runtime's JSON-RPC protocol.
//! It never retries a request: a failed or cancelled turn may have run remotely.
use super::{RuntimeInfo, check_protocol_version, connect_rpc, ping_rpc, read_rpc_frame};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

const MAX_REQUEST: usize = 1024 * 1024;
const MAX_INTERLEAVED_MESSAGES: usize = 100_000;
const MAX_BUFFERED_EVENTS: usize = 1024;
const MAX_BUFFERED_BYTES: usize = 1024 * 1024;

enum Reply {
    Result(Value),
    MethodNotFound,
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    ensure!(body.len() <= MAX_REQUEST, "Copilot request too large");
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

async fn request(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    id: u64,
    method: &str,
    params: Value,
    buffered: Option<(&str, &mut Vec<Value>)>,
) -> Result<Reply> {
    let mut buffered = buffered;
    let mut buffered_bytes = 0usize;
    write_frame(
        writer,
        &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
    )
    .await?;
    for _ in 0..MAX_INTERLEAVED_MESSAGES {
        let message = read_rpc_frame(reader).await?;
        // No host-side tools or permissions are registered. Reject runtime
        // requests instead of silently granting access or blocking the turn.
        if message.get("method").is_some() {
            if let Some(request_id) = message.get("id") {
                write_frame(writer, &json!({"jsonrpc":"2.0", "id":request_id, "error":{"code":-32601, "message":"Host method unavailable"}})).await?;
            } else if let Some((session, events)) = buffered.as_mut()
                && message["method"] == "session.event"
                && message["params"]["sessionId"] == *session
            {
                let event = &message["params"]["event"];
                buffered_bytes = buffered_bytes.saturating_add(serde_json::to_vec(event)?.len());
                ensure!(
                    events.len() < MAX_BUFFERED_EVENTS && buffered_bytes <= MAX_BUFFERED_BYTES,
                    "too many buffered Copilot events"
                );
                events.push(event.clone());
            }
            continue;
        }
        if message.get("id") != Some(&json!(id)) {
            continue;
        }
        // Runtime errors may contain credentials or prompts: do not echo them.
        if let Some(error) = message.get("error") {
            if error["code"].as_i64() == Some(-32601) {
                return Ok(Reply::MethodNotFound);
            }
            bail!("Copilot runtime rejected {method}");
        }
        return Ok(Reply::Result(
            message
                .get("result")
                .cloned()
                .context("Copilot response missing result")?,
        ));
    }
    bail!("too many messages while waiting for Copilot {method}")
}

fn required_reply(reply: Reply, method: &str) -> Result<Value> {
    match reply {
        Reply::Result(value) => Ok(value),
        Reply::MethodNotFound => bail!("Copilot runtime does not support {method}"),
    }
}

#[derive(Default)]
struct LegacyTurn {
    started: bool,
    response: Option<Value>,
}

impl LegacyTurn {
    fn observe(&mut self, event: &Value, message_id: &str) -> Result<Option<Value>> {
        if event.get("agentId").is_some_and(|id| !id.is_null()) {
            return Ok(None);
        }
        let data = &event["data"];
        let originating = data["originatingMessageId"].as_str();
        match event["type"].as_str() {
            Some("user.message") if data["messageId"] == message_id => self.started = true,
            Some("session.warning")
                if data["warningType"] == "policy_blocked" && originating == Some(message_id) =>
            {
                self.started = true
            }
            Some("assistant.message")
                if originating == Some(message_id) || (originating.is_none() && self.started) =>
            {
                self.response = Some(event.clone());
                if originating == Some(message_id) {
                    self.started = true;
                }
            }
            Some("session.error") if originating.is_none() || originating == Some(message_id) => {
                bail!("Copilot session failed")
            }
            Some("session.idle") if self.started && data["mode"] != "autopilot" => {
                ensure!(data["aborted"] != true, "Copilot turn was aborted");
                return Ok(Some(
                    self.response.take().context("empty Copilot response")?,
                ));
            }
            Some("session.autopilot_stopped") if data["requestId"] == message_id => {
                ensure!(
                    data["reason"] != "aborted" && data["reason"] != "error",
                    "Copilot turn did not complete"
                );
                return Ok(Some(
                    self.response.take().context("empty Copilot response")?,
                ));
            }
            _ => {}
        }
        Ok(None)
    }
}

async fn legacy_turn(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    session: &str,
    params: Value,
) -> Result<Value> {
    let mut events = Vec::new();
    let reply = request(
        reader,
        writer,
        4,
        "session.send",
        params,
        Some((session, &mut events)),
    )
    .await?;
    let reply = required_reply(reply, "session.send")?;
    let message_id = reply["messageId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .context("Copilot send omitted message ID")?;
    let mut state = LegacyTurn::default();
    for event in events {
        if let Some(response) = state.observe(&event, message_id)? {
            return Ok(response);
        }
    }
    for _ in 0..MAX_INTERLEAVED_MESSAGES {
        let message = read_rpc_frame(reader).await?;
        if let Some(request_id) = message.get("id")
            && message.get("method").is_some()
        {
            write_frame(writer, &json!({"jsonrpc":"2.0", "id":request_id, "error":{"code":-32601, "message":"Host method unavailable"}})).await?;
        } else if message["method"] == "session.event"
            && message["params"]["sessionId"] == session
            && let Some(response) = state.observe(&message["params"]["event"], message_id)?
        {
            return Ok(response);
        }
    }
    bail!("too many messages while waiting for Copilot completion")
}

#[cfg(test)]
async fn turn_rpc(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    model: &str,
    prompt: &str,
) -> Result<Value> {
    let session = uuid::Uuid::new_v4().to_string();
    validate_prompt(&session, prompt)?;
    open_session_rpc(reader, writer, &session, model, None).await?;
    send_message_rpc(reader, writer, &session, model, prompt).await
}

fn validate_prompt(session: &str, prompt: &str) -> Result<()> {
    let send_params = json!({"sessionId":session, "prompt":prompt});
    let send_request =
        json!({"jsonrpc":"2.0", "id":3, "method":"session.sendAndWait", "params":send_params});
    ensure!(
        serde_json::to_vec(&send_request)?.len() <= MAX_REQUEST,
        "Copilot request too large"
    );
    Ok(())
}

async fn open_session_rpc(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    session: &str,
    model: &str,
    reasoning_effort: Option<&str>,
) -> Result<()> {
    let version = match connect_rpc(reader, writer).await? {
        Some(version) => version,
        None => ping_rpc(reader, writer).await?,
    };
    check_protocol_version(version)?;
    let mut params = json!({
        "sessionId":session, "model":model, "requestPermission":false,
        "requestUserInput":false, "envValueMode":"direct",
        "includeSubAgentStreamingEvents":true
    });
    if let Some(effort) = reasoning_effort {
        params["reasoningEffort"] = json!(effort);
    }
    let created = request(reader, writer, 2, "session.create", params, None).await?;
    let created = required_reply(created, "session.create")?;
    ensure!(
        created["sessionId"] == session,
        "Copilot session identity mismatch"
    );
    Ok(())
}

async fn send_message_rpc(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
    session: &str,
    model: &str,
    prompt: &str,
) -> Result<Value> {
    validate_prompt(session, prompt)?;
    let send_params = json!({"sessionId":session, "prompt":prompt});
    let result = request(
        reader,
        writer,
        3,
        "session.sendAndWait",
        send_params.clone(),
        None,
    )
    .await?;
    let response = match result {
        Reply::Result(result) => {
            ensure!(result["aborted"] != true, "Copilot turn was aborted");
            result
                .get("response")
                .cloned()
                .context("empty Copilot response")?
        }
        // A method-not-found reply proves the runtime did not execute the
        // request. Other failures are never retried.
        Reply::MethodNotFound => legacy_turn(reader, writer, session, send_params).await?,
    };
    ensure!(
        response["type"] == "assistant.message"
            && response.get("agentId").is_none_or(Value::is_null),
        "Copilot returned a non-assistant response"
    );
    let content = response["data"]["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("empty Copilot response")?;
    Ok(json!({"content":content, "model":response["data"]["model"].as_str().unwrap_or(model)}))
}

/// Owns the runtime and its RPC streams for a single session. Dropping this
/// value stops the process group on Unix and the direct child on Windows.
pub(crate) struct NativeSession {
    _child: tokio::process::Child,
    _group: crate::process::ProcessGroup,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    pub(crate) id: String,
    pub(crate) model: String,
    usable: bool,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl NativeSession {
    pub(crate) async fn start(
        runtime: &RuntimeInfo,
        cwd: &Path,
        token: &str,
        model: &str,
        reasoning_effort: Option<&str>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<Self> {
        crate::cancellation::Cancellation::current().check()?;
        ensure!(!token.is_empty(), "Copilot token is required");
        let mut command = tokio::process::Command::new(&runtime.binary);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .args([
                "--headless",
                "--no-auto-update",
                "--stdio",
                "--auth-token-env",
                "COPILOT_SDK_AUTH_TOKEN",
                "--no-auto-login",
            ])
            .env("COPILOT_SDK_AUTH_TOKEN", token)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("start native Copilot runtime")?;
        let group = crate::process::ProcessGroup::new(&child);
        let stdin = child
            .stdin
            .take()
            .context("Copilot runtime stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Copilot runtime stdout unavailable")?;
        let mut session = Self {
            _child: child,
            _group: group,
            stdin,
            stdout,
            id: uuid::Uuid::new_v4().to_string(),
            model: model.to_owned(),
            usable: true,
            _permit: permit,
        };
        crate::cancellation::wait(tokio::time::timeout(
            Duration::from_secs(10),
            open_session_rpc(
                &mut session.stdout,
                &mut session.stdin,
                &session.id,
                &session.model,
                reasoning_effort,
            ),
        ))
        .await?
        .context("Copilot session creation timed out")??;
        Ok(session)
    }

    pub(crate) fn validate_request(&self, prompt: &str, timeout_ms: Option<u64>) -> Result<u64> {
        ensure!(!prompt.is_empty(), "Copilot prompt is required");
        ensure!(self.usable, "Copilot session is no longer usable");
        validate_prompt(&self.id, prompt)?;
        let timeout_ms = timeout_ms.unwrap_or(60_000);
        ensure!(
            (1..=300_000).contains(&timeout_ms),
            "invalid Copilot timeout"
        );
        Ok(timeout_ms)
    }

    pub(crate) async fn send(&mut self, prompt: &str, timeout_ms: Option<u64>) -> Result<Value> {
        let timeout_ms = self.validate_request(prompt, timeout_ms)?;
        // Until a reply has been fully read, a retry cannot safely assume the
        // runtime did not act on the previous message.
        self.usable = false;
        let result = crate::cancellation::wait(tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            send_message_rpc(
                &mut self.stdout,
                &mut self.stdin,
                &self.id,
                &self.model,
                prompt,
            ),
        ))
        .await?
        .context("Copilot turn timed out; inspect its state before retrying")?;
        if result.is_ok() {
            self.usable = true;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Child process used by the lifecycle test. FD 3 carries protocol output;
    /// the test harness's normal stdout is redirected away from the protocol.
    #[cfg(unix)]
    #[test]
    fn fake_runtime_child() {
        if std::env::var("COPILOT_SDK_AUTH_TOKEN").as_deref() != Ok("native-session-test-token") {
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut input = tokio::io::stdin();
            let output = std::fs::OpenOptions::new().write(true).open("/dev/fd/3").unwrap();
            let mut output = tokio::fs::File::from_std(output);
            assert_eq!(read_rpc_frame(&mut input).await.unwrap()["method"], "connect");
            write_frame(&mut output, &json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}})).await.unwrap();
            let created = read_rpc_frame(&mut input).await.unwrap();
            assert_eq!(created["method"], "session.create");
            assert_eq!(created["params"]["model"], "chosen-model");
            assert_eq!(created["params"]["reasoningEffort"], "high");
            let session = created["params"]["sessionId"].clone();
            write_frame(&mut output, &json!({"jsonrpc":"2.0", "id":2, "result":{"sessionId":session}})).await.unwrap();
            loop {
                let sent = read_rpc_frame(&mut input).await.unwrap();
                assert_eq!(sent["params"]["sessionId"], session);
                let prompt = sent["params"]["prompt"].as_str().unwrap();
                if prompt == "hang" {
                    tokio::fs::write("hang-started", b"ready").await.unwrap();
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    break;
                }
                if prompt == "fail" {
                    write_frame(&mut output, &json!({"jsonrpc":"2.0", "id":3, "error":{"code":-32000, "message":"failure"}})).await.unwrap();
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    break;
                }
                let answer = match prompt { "first" => "one", "second" => "two", _ => panic!("unexpected prompt") };
                write_frame(&mut output, &json!({"jsonrpc":"2.0", "id":3, "result":{"aborted":false, "response":{"type":"assistant.message", "data":{"content":answer}}}})).await.unwrap();
            }
        });
    }

    #[cfg(unix)]
    async fn test_session() -> (std::path::PathBuf, crate::Repository, String) {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("copilot-session-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let executable = std::env::current_exe().unwrap();
        let quoted = executable.to_string_lossy().replace('\'', "'\\''");
        let wrapper = root.join("runtime");
        std::fs::write(&wrapper, format!("#!/bin/sh\nexec 3>&1\nexec '{quoted}' --exact copilot::native_turn::tests::fake_runtime_child --nocapture >/dev/null\n")).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let info = RuntimeInfo {
            root: root.clone(),
            binary: wrapper,
            platform: "test".into(),
            arch: "test".into(),
        };
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap();
        assert!(status.success());
        let repo = crate::Repository::open(&root).await.unwrap();
        let handle = repo
            .create_native_copilot_session(
                &info,
                "native-session-test-token",
                Some("chosen-model"),
                Some("high"),
            )
            .await
            .unwrap();
        assert_eq!(handle["model"], "chosen-model");
        let id = handle["id"].as_str().unwrap().to_owned();
        (root, repo, id)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_process_survives_two_turns_and_is_owned_by_session() {
        let (root, repo, id) = test_session().await;
        assert!(
            repo.execute(crate::Operation::CopilotSendMessage {
                session_id: id.clone(),
                message: "first".into(),
                timeout_ms: Some(0),
            })
            .await
            .is_err()
        );
        assert_eq!(repo.native_copilot_sessions.lock().await.len(), 1);
        assert!(
            repo.execute(crate::Operation::CopilotSendMessage {
                session_id: id.clone(),
                message: String::new(),
                timeout_ms: None,
            })
            .await
            .is_err()
        );
        assert_eq!(repo.native_copilot_sessions.lock().await.len(), 1);
        for (prompt, answer) in [("first", "one"), ("second", "two")] {
            let result = repo
                .clone()
                .execute(crate::Operation::CopilotSendMessage {
                    session_id: id.clone(),
                    message: prompt.to_owned(),
                    timeout_ms: None,
                })
                .await
                .unwrap();
            assert_eq!(result["content"], answer);
            assert_eq!(result["model"], "chosen-model");
        }
        assert_eq!(
            repo.execute(crate::Operation::CopilotCloseSession { session_id: id })
                .await
                .unwrap()["closed"],
            true
        );
        assert!(repo.native_copilot_sessions.lock().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_turn_discards_the_session_without_replay() {
        let (root, repo, id) = test_session().await;
        let result = repo
            .execute(crate::Operation::CopilotSendMessage {
                session_id: id,
                message: "fail".into(),
                timeout_ms: None,
            })
            .await;
        assert!(result.is_err());
        assert!(repo.native_copilot_sessions.lock().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_capacity_is_reserved_before_starting_another_process() {
        let (root, repo, id) = test_session().await;
        let held = repo
            .native_copilot_slots
            .clone()
            .try_acquire_many_owned((crate::copilot_bridge::MAX_NATIVE_SESSIONS - 1) as u32)
            .unwrap();
        let missing = RuntimeInfo {
            root: root.clone(),
            binary: root.join("missing"),
            platform: "test".into(),
            arch: "test".into(),
        };
        let error = repo
            .create_native_copilot_session(&missing, "test", None, None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("too many active Copilot sessions")
        );
        drop(held);
        repo.execute(crate::Operation::CopilotCloseSession { session_id: id })
            .await
            .unwrap();
        assert_eq!(
            repo.native_copilot_slots.available_permits(),
            crate::copilot_bridge::MAX_NATIVE_SESSIONS
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_turn_removes_and_stops_the_session() {
        let (root, repo, id) = test_session().await;
        let slot = repo
            .native_copilot_sessions
            .lock()
            .await
            .get(&id)
            .unwrap()
            .clone();
        let pid = slot.lock().await.as_ref().unwrap()._child.id().unwrap();
        let cancellation = crate::cancellation::Cancellation::default();
        let worker_token = cancellation.clone();
        let worker_repo = repo.clone();
        let job = tokio::spawn(worker_token.scope(async move {
            worker_repo
                .execute(crate::Operation::CopilotSendMessage {
                    session_id: id,
                    message: "hang".into(),
                    timeout_ms: None,
                })
                .await
        }));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !root.join("hang-started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(repo.native_copilot_sessions.lock().await.is_empty());
        #[cfg(target_os = "linux")]
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Err(_) => break,
                    Ok(stat)
                        if stat
                            .rsplit_once(')')
                            .is_some_and(|(_, rest)| rest.starts_with(" Z ")) =>
                    {
                        break;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .expect("cancelled runtime remained running");
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_turn_never_restores_the_uncertain_session() {
        let (root, repo, id) = test_session().await;
        let worker = repo.clone();
        let worker_id = id.clone();
        let job = tokio::spawn(async move {
            worker
                .execute(crate::Operation::CopilotSendMessage {
                    session_id: worker_id,
                    message: "hang".into(),
                    timeout_ms: None,
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !root.join("hang-started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        let slot = repo
            .native_copilot_sessions
            .lock()
            .await
            .get(&id)
            .unwrap()
            .clone();
        assert!(slot.lock().await.is_none());
        repo.execute(crate::Operation::CopilotCloseSession { session_id: id })
            .await
            .unwrap();
        assert!(repo.native_copilot_sessions.lock().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn one_connection_can_send_multiple_turns_to_the_same_session() {
        let (client, mut server) = tokio::io::duplex(8192);
        let (mut reader, mut writer) = tokio::io::split(client);
        let peer = tokio::spawn(async move {
            assert_eq!(
                read_rpc_frame(&mut server).await.unwrap()["method"],
                "connect"
            );
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}}),
            )
            .await
            .unwrap();
            let create = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(create["params"]["sessionId"], "session-1");
            assert_eq!(create["params"]["model"], "chosen-model");
            assert_eq!(create["params"]["reasoningEffort"], "high");
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":2, "result":{"sessionId":"session-1"}}),
            )
            .await
            .unwrap();
            for (prompt, answer) in [("first", "one"), ("second", "two")] {
                let send = read_rpc_frame(&mut server).await.unwrap();
                assert_eq!(send["method"], "session.sendAndWait");
                assert_eq!(send["params"]["sessionId"], "session-1");
                assert_eq!(send["params"]["prompt"], prompt);
                write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":3, "result":{"aborted":false, "response":{"type":"assistant.message", "data":{"content":answer}}}})).await.unwrap();
            }
        });
        open_session_rpc(
            &mut reader,
            &mut writer,
            "session-1",
            "chosen-model",
            Some("high"),
        )
        .await
        .unwrap();
        assert_eq!(
            send_message_rpc(
                &mut reader,
                &mut writer,
                "session-1",
                "chosen-model",
                "first"
            )
            .await
            .unwrap()["content"],
            "one"
        );
        assert_eq!(
            send_message_rpc(
                &mut reader,
                &mut writer,
                "session-1",
                "chosen-model",
                "second"
            )
            .await
            .unwrap()["content"],
            "two"
        );
        peer.await.unwrap();
    }

    #[test]
    fn legacy_completion_rejects_abort_and_ignores_unrelated_events() {
        let mut state = LegacyTurn::default();
        assert!(
            state
                .observe(
                    &json!({"type":"user.message", "data":{"messageId":"other"}}),
                    "m1"
                )
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .observe(&json!({"type":"session.idle", "data":{}}), "m1")
                .unwrap()
                .is_none()
        );
        state
            .observe(
                &json!({"type":"user.message", "data":{"messageId":"m1"}}),
                "m1",
            )
            .unwrap();
        state.observe(&json!({"type":"assistant.message", "data":{"originatingMessageId":"m1", "content":"answer"}}), "m1").unwrap();
        assert!(
            state
                .observe(
                    &json!({"type":"session.idle", "data":{"aborted":true}}),
                    "m1"
                )
                .is_err()
        );
    }

    #[tokio::test]
    async fn legacy_runtime_buffers_early_events_and_correlates_the_turn() {
        let (client, mut server) = tokio::io::duplex(8192);
        let (mut reader, mut writer) = tokio::io::split(client);
        let peer = tokio::spawn(async move {
            read_rpc_frame(&mut server).await.unwrap();
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}}),
            )
            .await
            .unwrap();
            let create = read_rpc_frame(&mut server).await.unwrap();
            let session = create["params"]["sessionId"].clone();
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":2, "result":{"sessionId":session}}),
            )
            .await
            .unwrap();
            assert_eq!(
                read_rpc_frame(&mut server).await.unwrap()["method"],
                "session.sendAndWait"
            );
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":3, "error":{"code":-32601, "message":"method not found"}})).await.unwrap();
            assert_eq!(
                read_rpc_frame(&mut server).await.unwrap()["method"],
                "session.send"
            );
            for (sid, event) in [
                (
                    json!("other"),
                    json!({"type":"assistant.message", "data":{"originatingMessageId":"m1", "content":"wrong"}}),
                ),
                (
                    session.clone(),
                    json!({"type":"user.message", "data":{"messageId":"m1"}}),
                ),
                (
                    session.clone(),
                    json!({"type":"assistant.message", "data":{"originatingMessageId":"m1", "content":"legacy answer"}}),
                ),
            ] {
                write_frame(&mut server, &json!({"jsonrpc":"2.0", "method":"session.event", "params":{"sessionId":sid, "event":event}})).await.unwrap();
            }
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":4, "result":{"messageId":"m1"}}),
            )
            .await
            .unwrap();
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "method":"session.event", "params":{"sessionId":session, "event":{"type":"session.idle", "data":{}}}})).await.unwrap();
        });
        let result = turn_rpc(&mut reader, &mut writer, "model", "prompt")
            .await
            .unwrap();
        assert_eq!(result["content"], "legacy answer");
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_prompt_fails_before_any_request_is_sent() {
        let (client, mut server) = tokio::io::duplex(1024);
        let (mut reader, mut writer) = tokio::io::split(client);
        let error = turn_rpc(&mut reader, &mut writer, "model", &"x".repeat(MAX_REQUEST))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("too large"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), read_rpc_frame(&mut server))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn native_turn_correlates_replies_and_denies_host_requests() {
        let (client, mut server) = tokio::io::duplex(8192);
        let (mut reader, mut writer) = tokio::io::split(client);
        let server_task = tokio::spawn(async move {
            let connect = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(connect["method"], "connect");
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}}),
            )
            .await
            .unwrap();
            let create = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(create["method"], "session.create");
            assert!(create["params"].get("reasoningEffort").is_none());
            let session = create["params"]["sessionId"].clone();
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "method":"session.event", "params":{}}),
            )
            .await
            .unwrap();
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":2, "result":{"sessionId":session}}),
            )
            .await
            .unwrap();
            let send = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(send["method"], "session.sendAndWait");
            assert_eq!(send["params"]["prompt"], "private prompt");
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":"server-1", "method":"permission.request", "params":{}})).await.unwrap();
            let denied = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(denied["error"]["code"], -32601);
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":3, "result":{"aborted":false, "response":{"type":"assistant.message", "data":{"content":"answer"}}}})).await.unwrap();
        });
        let result = turn_rpc(&mut reader, &mut writer, "model", "private prompt")
            .await
            .unwrap();
        assert_eq!(result["content"], "answer");
        assert_eq!(result["model"], "model");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn native_turn_does_not_expose_runtime_error_or_retry_a_failed_send() {
        let (client, mut server) = tokio::io::duplex(8192);
        let (mut reader, mut writer) = tokio::io::split(client);
        let server_task = tokio::spawn(async move {
            read_rpc_frame(&mut server).await.unwrap();
            write_frame(
                &mut server,
                &json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}}),
            )
            .await
            .unwrap();
            let create = read_rpc_frame(&mut server).await.unwrap();
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":2, "result":{"sessionId":create["params"]["sessionId"]}})).await.unwrap();
            assert_eq!(
                read_rpc_frame(&mut server).await.unwrap()["method"],
                "session.sendAndWait"
            );
            write_frame(&mut server, &json!({"jsonrpc":"2.0", "id":3, "error":{"code":-32000, "message":"secret-token"}})).await.unwrap();
            let next =
                tokio::time::timeout(Duration::from_millis(100), read_rpc_frame(&mut server)).await;
            assert!(
                next.is_err() || next.unwrap().is_err(),
                "turn was unexpectedly replayed"
            );
        });
        let error = turn_rpc(&mut reader, &mut writer, "model", "prompt")
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret-token"));
        server_task.await.unwrap();
    }
}
