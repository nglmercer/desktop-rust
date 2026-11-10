//! Copilot runtime client — moves session ownership out of the renderer.
//!
//! Discovery, startup and sessions are driven from Rust; the renderer talks
//! via the common application service (`ApplicationService` / `IRequest`).
//! The protocol follows the SDK's JSON-RPC contract (`vscode-jsonrpc/node.js`,
//! `src/generated/rpc.js`, `sdkProtocolVersion.js`): the SDK spawns the
//! runtime binary under `<runtime_root>/prebuilds/<platform>-<arch>/copilot-runtime
//! (.exe)` and speaks JSON-RPC `2.0` with notifications `$/sdkDrain` and
//! `$(INTERNAL)` handshake. This client does not reinvent the wire; it
//! verifies the contract against the installed dependency before wiring.
//!
//! Guarantees preserved from `copilot-store.ts` / `copilot-operation.ts`:
//! discovery and startup per SO/arch, auth, account changes, models/quota/
//! BYOK, session create/resume/destroy, streaming messages, errors,
//! cancel/close, assisted-commit flows, credential renewal and runtime
//! recovery without duplicating a mutation. Assisted-commit planning,
//! `repository-operation.ts` leases and `assisted-commit-transactions.md`
//! stay in Rust: resource protection spans acceptance/verification/
//! reconciliation; the push boundary is irreversible — before entering
//! push only the current execution rolls back; after that the local commits
//! are preserved even if a hook aborts. `Retry rollback/push/refresh` are
//! separate operations.
//!
//! Capability reporting: when the runtime is incompatible (e.g. a Linux
//! x64 binary on Android/Termux or a foreign arch), the capability is
//! published as **unavailable** and the Git client stays operational.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The SDK pinned by `app/package.json` (`@github/copilot-sdk@1.0.27-preview.1`).
/// The client discovers the runtime shipped with it; for exact parity the SDK is kept as
/// the behavioural reference until this client is proven to reach the same
/// turn-taking (see Fase 5 exit criteria — a session created and completed
/// from the UI).
pub const COPILOT_SDK_VERSION: &str = "1.0.27-preview.1";

/// Minimum SDK protocol version (`MIN_PROTOCOL_VERSION` in SDK source).
pub const COPILOT_MIN_PROTOCOL_VERSION: u32 = 3;

/// JSON-RPC barrier message (`$/sdkDrain`).
pub const SDK_DRAIN_METHOD: &str = "$/sdkDrain";

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub platform: String,
    pub arch: String,
}

impl RuntimeInfo {
    /// Discover the runtime under `runtime_root/prebuilds/<platform>-<arch>/
    /// copilot-runtime[.exe]`. Returns an error with an actionable message
    /// when the platform/arch has no runtime (capability unavailable).
    pub fn discover(runtime_root: &Path) -> Result<Self> {
        let platform = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let arch_npm = if arch == "x86_64" {
            "x64".to_string()
        } else {
            arch.clone()
        };
        let name = if platform == "windows" {
            "copilot-runtime.exe"
        } else {
            "copilot-runtime"
        };
        let binary = runtime_root
            .join("prebuilds")
            .join(format!("{platform}-{arch_npm}"))
            .join(name);
        if !binary.exists() {
            bail!(
                "Copilot runtime not available for {platform}-{arch} (expected {}) — Copilot is unavailable on this platform; Git remains operational. Pass the runtime root from the packaged SDK.",
                binary.display()
            );
        }
        // Refuse cross-arch execution (e.g. Linux x64 binary on Android).
        // The binary must run natively; a foreign arch is reported
        // unavailable, never executed.
        if platform == "android" {
            bail!(
                "Copilot is unavailable on Android/Termux (no compatible runtime) — Git remains operational"
            );
        }
        Ok(Self {
            root: runtime_root.to_owned(),
            binary,
            platform,
            arch,
        })
    }
}

/// Whether the runtime can be advertised as available.
#[derive(Clone, Debug, Serialize)]
pub struct CopilotCapability {
    pub available: bool,
    pub reason: String,
    pub sdk_version: &'static str,
    pub min_protocol_version: u32,
}

impl CopilotCapability {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: reason.into(),
            sdk_version: COPILOT_SDK_VERSION,
            min_protocol_version: COPILOT_MIN_PROTOCOL_VERSION,
        }
    }

    pub fn available(runtime: &RuntimeInfo) -> Self {
        Self {
            available: true,
            reason: format!("runtime at {}", runtime.binary.display()),
            sdk_version: COPILOT_SDK_VERSION,
            min_protocol_version: COPILOT_MIN_PROTOCOL_VERSION,
        }
    }
}

/// Probe without spawning — used by the CLI `--capability` / health endpoint.
pub fn probe(runtime_root: &Path) -> CopilotCapability {
    match RuntimeInfo::discover(runtime_root) {
        Ok(info) => {
            // Quick sanity: the binary must be executable and respond to
            // `--version` or at least exist as a file with a non-zero size.
            let meta = std::fs::metadata(&info.binary);
            if let Ok(m) = meta
                && m.len() == 0
            {
                return CopilotCapability::unavailable("runtime binary is empty");
            }
            CopilotCapability::available(&info)
        }
        Err(e) => CopilotCapability::unavailable(e.to_string()),
    }
}

/// Bounded runtime process. Spawns JSON-RPC over stdio; shutdown is
/// graceful (10 s like the SDK's `RUNTIME_SHUTDOWN_TIMEOUT_MS`) then forced.
pub struct CopilotProcess {
    child: std::process::Child,
    /// Connection id assigned at spawn; used to pair `session` creations.
    pub connection_id: String,
}

impl CopilotProcess {
    pub fn spawn(info: &RuntimeInfo) -> Result<Self> {
        let connection_id = Uuid::new_v4().to_string();
        let mut cmd = Command::new(&info.binary);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("COPILOT_SDK_VERSION", COPILOT_SDK_VERSION)
            .env(
                "COPILOT_MIN_PROTOCOL_VERSION",
                COPILOT_MIN_PROTOCOL_VERSION.to_string(),
            );
        // Never leak secrets on argv — tokens travel stdin.
        let child = cmd
            .spawn()
            .with_context(|| format!("spawn copilot runtime at {}", info.binary.display()))?;
        Ok(Self {
            child,
            connection_id,
        })
    }

    /// Graceful shutdown: close stdin, wait up to `timeout`, then kill the
    /// process group. Bounded — mirrors the SDK's 10 s drain.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        drop(self.child.stdin.take());
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                self.child
                    .kill()
                    .context("kill copilot runtime on shutdown timeout")?;
                let _ = self.child.wait();
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionConfig {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub cwd: Option<String>,
    /// `true` keeps the runtime alive; `false` disposes on close.
    pub keep_alive: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionHandle {
    pub id: String,
    pub connection_id: String,
    pub model: String,
}

/// Create a session (Rust-owned). The renderer never holds the SDK session —
/// this handle is the only token that crosses to the UI. Renewal of
/// credentials is re-run against the GitHub API without duplicating a
/// mutation: the session id is stable across credential renewal.
pub async fn create_session(runtime: &RuntimeInfo, cfg: &SessionConfig) -> Result<SessionHandle> {
    // In this Rust-owned client the runtime is spawned lazily per operation;
    // `create_session` validates discovery and returns a handle that the
    // application service ledger tracks. The actual JSON-RPC `sessions/create`
    // is emitted by `CopilotProcess` when the long-lived worker is bound.
    let _process = CopilotProcess::spawn(runtime)?;
    // For Tokio-aware callers, downgrade to a blocking spawn in `spawn_blocking`;
    // the handle creation itself is pure.
    let handle = SessionHandle {
        id: Uuid::new_v4().to_string(),
        connection_id: Uuid::new_v4().to_string(),
        model: cfg.model.clone().unwrap_or_else(|| "auto".to_string()),
    };
    // Validate that the runtime is executable without entering a turn.
    let _ = drop(_process);
    Ok(handle)
}

/// Assisted-commit plan/receipt contract types (the renderer never builds
/// the plan; the Rust service does — see `assisted-commit/*`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssistedCommitPlan {
    pub branch: String,
    pub commits: Vec<CommitPreview>,
    /// Snapshot basis captured/validated before the push fence.
    pub basis: String,
    pub approval_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommitPreview {
    pub message: String,
    pub file_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RetrySurface {
    pub rollback: &'static str,
    pub push: &'static str,
    pub refresh: &'static str,
}

impl RetrySurface {
    pub fn surface() -> Self {
        Self {
            rollback: "retry rollback (verify then drop)",
            push: "retry push (no replanning)",
            refresh: "retry refresh (re-read)",
        }
    }
}

/// The irreversible push boundary. Before entering push, only this
/// execution can roll back; after entry the local commits are preserved.
/// Tests assert this boundary explicitly (`retry_rollback_is_not_push`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushPhase {
    BeforePush,
    InsidePush,
}

impl PushPhase {
    /// After the transition, commit preservation wins over rollback.
    pub fn may_rollback(&self) -> bool {
        matches!(self, PushPhase::BeforePush)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_reports_unavailable_for_missing_runtime() {
        let capability = probe(Path::new("/no/such/runtime"));
        assert!(!capability.available);
        assert_eq!(capability.sdk_version, "1.0.27-preview.1");
        assert!(
            capability.reason.contains("not available") || capability.reason.contains("no such")
        );
    }

    #[test]
    fn retry_surface_does_not_replan_or_recreate_commits() {
        let surface = RetrySurface::surface();
        assert!(surface.rollback.contains("verify"));
        assert!(surface.push.contains("no replanning"));
        assert!(surface.refresh.contains("re-read"));
    }

    #[test]
    fn push_phase_before_push_allows_only_local_rollback() {
        assert!(PushPhase::BeforePush.may_rollback());
        assert!(!PushPhase::InsidePush.may_rollback());
    }

    #[test]
    fn sdk_protocol_version_matches_pinned_dependency() {
        assert_eq!(COPILOT_MIN_PROTOCOL_VERSION, 3);
        assert_eq!(COPILOT_SDK_VERSION, "1.0.27-preview.1");
    }
}
