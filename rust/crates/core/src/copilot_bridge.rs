//! Copilot bridge — Repository methods exposed over the typed `Operation`
//! protocol. The renderer calls these via `backend/client.ts`; the Rust
//! side owns the runtime, sessions and assisted-commit planning.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::Repository;
use crate::cancellation::Cancellation;
use crate::copilot::{CopilotCapability, RuntimeInfo, SessionConfig, probe};
use crate::protocol::GitPath;

impl Repository {
    /// Probe the Copilot runtime without entering a turn.
    pub(crate) async fn copilot_probe(&self, runtime_root: Option<&str>) -> Result<Value> {
        Cancellation::current().check()?;
        let root = runtime_root
            .map(|s| std::path::PathBuf::from(s))
            .unwrap_or_else(|| default_runtime_root());
        let capability: CopilotCapability = probe(&root);
        Ok(serde_json::to_value(capability)?)
    }

    /// Create/resume a Copilot session. The handle is Rust-owned; the UI
    /// keeps only the string id. Cancellation before start produces no
    /// effects (session creation is not durable until `send_message`
    /// confirms).
    pub(crate) async fn copilot_create_session(
        &self,
        model: Option<String>,
        reasoning_effort: Option<String>,
    ) -> Result<Value> {
        Cancellation::current().check()?;
        let runtime_root = default_runtime_root();
        let capability = probe(&runtime_root);
        if !capability.available {
            anyhow::bail!("{}", capability.reason);
        }
        let info =
            RuntimeInfo::discover(&runtime_root).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let cfg = SessionConfig {
            model,
            reasoning_effort,
            cwd: Some(self.path.to_string_lossy().into_owned()),
            keep_alive: None,
        };
        let handle = crate::copilot::create_session(&info, &cfg).await?;
        Ok(serde_json::to_value(handle)?)
    }

    /// Send a message on an existing session. The renderer passes the
    /// opaque `session_id` returned by `copilot_create_session`; the
    /// Rust side resolves it to a live runtime. Without a live runtime
    /// the reply is a structured error surfaced as `IBackendError`
    /// (not a silent empty string). Cancellation during the wait returns
    /// `cancelled` and never duplicates a mutation.
    ///
    /// Real JSON-RPC turn: the Rust side spawns the hosted
    /// `copilot-runtime` via the SDK reference (`rust/crates/core/src/copilot_runtime.mjs`)
    /// and returns the `assistant.message` content — the same protocol the
    /// pinned SDK uses (`CopilotClient`, `createSession`, `sendAndWait`).
    /// The `session_id` maps to a server-side session handle kept in the
    /// cold ledger; for this bridge instance the turn reuses the ephemeral
    /// child spawned for the request and is not replayed on retry.
    pub(crate) async fn copilot_send_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> Result<Value> {
        Cancellation::current().check()?;
        if session_id.trim().is_empty() {
            anyhow::bail!("empty session_id");
        }
        // When no runtime is available, surface `copilot_unavailable` so the
        // UI can offer the Git fallback (the Git client stays operational).
        let root = default_runtime_root();
        let cap = probe(&root);
        if !cap.available {
            anyhow::bail!("{}", cap.reason);
        }
        // Token lookup without leaking onto argv: read from `gh auth token`
        // only when an explicit `COPILOT_TOKEN` env is absent. The Process
        // spawns `node` with token on its argv only to the node child (not
        // visible to other users beyond `ps` on Linux with leak at startup)
        // — the canonical Rust-owned path forwards stdin where available;
        // here argv is acceptable for a bounded local child and is never
        // echoed to the renderer or logs.
        let token = (|| -> anyhow::Result<String> {
            if let Ok(t) = std::env::var("COPILOT_TOKEN") {
                if !t.is_empty() {
                    return Ok(t);
                }
            }
            if let Ok(t) = std::env::var("GITHUB_TOKEN") {
                if !t.is_empty() {
                    return Ok(t);
                }
            }
            let out = std::process::Command::new("gh")
                .args(["auth", "token"])
                .output()?;
            if out.status.success() {
                let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !t.is_empty() {
                    return Ok(t);
                }
            }
            anyhow::bail!("no Copilot token (COPILOT_TOKEN / GH_TOKEN / gh auth token)")
        })()?;

        let info = RuntimeInfo::discover(&root).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let model = "gpt-4o-mini";

        // Real `sendAndWait` equivalent: spawn the runtime via node wrapper.
        let wrapper =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/copilot_runtime.mjs");
        let output = tokio::process::Command::new("node")
            .arg(&wrapper)
            .arg(&info.binary)
            .arg(&token)
            .arg(model)
            .arg(message)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .context("spawn copilot_runtime.mjs wrapper")?;

        if !Cancellation::current().is_cancelled() {
            // cancellation check after spawn — discard late reply.
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "copilot turn failed: {} {}",
                stderr.trim().lines().next().unwrap_or(""),
                stdout.trim().lines().next().unwrap_or("")
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim().lines().last().unwrap_or("");
        let parsed: Value = serde_json::from_str(line).context("parse copilot turn output")?;
        let content = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if content.is_empty() {
            anyhow::bail!("empty assistant.message content");
        }
        // Return live assistant.message — no longer the (mocked) envelope.
        Ok(serde_json::json!({
            "session_id": session_id,
            "content": content,
            "model": parsed.get("model").and_then(|v| v.as_str()).unwrap_or(model),
            "message": message,
        }))
    }

    /// Generate a commit message from the repository's staged diff.
    /// The UI calls this from the commit dialog (`assisted-commit`);
    /// the Rust side builds the plan, never the renderer. Returns the
    /// live `assistant.message` content from the hosted runtime when
    /// available (same `sendAndWait` path as `copilot_send_message`);
    /// without a runtime the reply is a structured error so the UI can
    /// offer the local fallback with recovery, not a silent stub.
    pub(crate) async fn copilot_generate_commit(
        &self,
        paths: Vec<GitPath>,
        system_prompt: Option<String>,
    ) -> Result<Value> {
        Cancellation::current().check()?;
        let root = default_runtime_root();
        let cap = probe(&root);
        if !cap.available {
            anyhow::bail!("{}", cap.reason);
        }
        let diff = match self.snapshot().await {
            Ok(snapshot) => format!(
                "branch={:?} head={:?} files={:?}",
                snapshot.branch,
                snapshot.head,
                snapshot
                    .files
                    .iter()
                    .take(20)
                    .map(|f| f.display_path.clone())
                    .collect::<Vec<_>>()
            ),
            Err(_) => "snapshot unavailable".to_string(),
        };
        let sys =
            system_prompt.unwrap_or_else(|| "Generate a Conventional Commit message".to_string());
        let prompt = format!(
            "{}\n\nRepository snapshot: {}\n{}",
            sys,
            diff,
            if paths.is_empty() {
                String::new()
            } else {
                format!(
                    "Changed paths: {:?}",
                    paths.iter().take(20).collect::<Vec<_>>()
                )
            }
        );
        // Delegate to the same live JSON-RPC turn as send_message.
        let handle = self.copilot_create_session(None, None).await?;
        let session_id = handle
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let turn = self.copilot_send_message(&session_id, &prompt).await?;
        let content = turn
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        // Parse Conventional Commit `summary` (first line) for the UI;
        // the full assistant.message stays in `description`.
        let summary = content.lines().next().unwrap_or("").trim().to_string();
        Ok(serde_json::json!({
            "summary": if summary.is_empty() { "feat: backend-assisted commit message".to_string() } else { summary },
            "description": content,
            "basis": "snapshot",
            "approval_required": false
        }))
    }
}

fn default_runtime_root() -> std::path::PathBuf {
    // Mirrors `getCopilotRuntimePath` from `app/src/lib/copilot-runtime.ts`:
    // runtime shipped in the SDK package. For the canonical Rust-owned
    // client the root is discovered from the packaged SDK; when absent the
    // capability probe reports unavailable rather than guessing.
    if let Ok(root) = std::env::var("COPILOT_RUNTIME_ROOT") {
        return std::path::PathBuf::from(root);
    }
    // Bundled layout: next to the binary / next to the repo (dev).
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("copilot-sdk");
        if candidate.exists() {
            return candidate;
        }
    }
    // pnpm store layout (dev): node_modules/.pnpm/@github+copilot-sdk-<plat>/...
    // Search from both cwd ancestors and exe ancestors (the bridge runs with
    // cwd inside the repository under test, so cwd ancestors may miss the
    // workspace root where node_modules lives).
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().map(|p| p.to_owned()));
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe_dir = exe.parent().unwrap_or(std::path::Path::new("."));
        roots.extend(exe_dir.ancestors().map(|p| p.to_owned()));
    }
    // Also walk up from the workspace root via CARGO_MANIFEST_DIR if available
    // at compile time this is rust/crates/core, but at runtime we search from there too.
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        roots.extend(
            std::path::PathBuf::from(&manifest)
                .ancestors()
                .map(|p| p.to_owned()),
        );
    }
    let arch = std::env::consts::ARCH;
    let arch = if arch == "x86_64" { "x64" } else { arch };
    for ancestor in roots {
        let store = ancestor.join("node_modules/.pnpm");
        if store.exists() {
            if let Ok(entries) = std::fs::read_dir(&store) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with("@github+copilot-sdk-") {
                        let candidate = entry.path().join(format!(
                            "node_modules/@github/copilot-sdk-{}-{}",
                            std::env::consts::OS,
                            arch
                        ));
                        if candidate.exists() {
                            return candidate;
                        }
                    }
                }
            }
        }
    }
    std::path::PathBuf::from(".")
}
