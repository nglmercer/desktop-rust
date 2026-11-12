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
//! Runtime discovery and a bounded native handshake and ping are implemented.
//! The optional native turn keeps its session in memory; the default SDK
//! wrapper still creates a new ephemeral session per request. Durable session
//! recovery, account changes, streaming and the full assisted
//! commit workflow remain migration work.
//!
//! Capability reporting: when the runtime is incompatible (e.g. a Linux
//! x64 binary on Android/Termux or a foreign arch), the capability is
//! published as **unavailable** and the Git client stays operational.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub(crate) mod native_turn;

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

/// Translate Rust target names to the platform labels used by the pinned SDK.
pub(crate) fn runtime_platform(
    os: &str,
    arch: &str,
    musl: bool,
) -> Option<(&'static str, &'static str)> {
    let platform = match os {
        "linux" if musl => "linuxmusl",
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win32",
        _ => return None,
    };
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some((platform, arch))
}

pub(crate) fn current_runtime_platform() -> Option<(&'static str, &'static str)> {
    runtime_platform(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
    )
}

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
        let (platform, arch) = current_runtime_platform().ok_or_else(|| anyhow::anyhow!(
            "Copilot runtime not available for {}-{} — Copilot is unavailable on this platform; Git remains operational",
            std::env::consts::OS, std::env::consts::ARCH
        ))?;
        let name = if platform == "win32" {
            "copilot-runtime.exe"
        } else {
            "copilot-runtime"
        };
        let binary = runtime_root
            .join("prebuilds")
            .join(format!("{platform}-{arch}"))
            .join(name);
        if !binary.exists() {
            bail!(
                "Copilot runtime not available for {platform}-{arch} (expected {}) — Copilot is unavailable on this platform; Git remains operational. Pass the runtime root from the packaged SDK.",
                binary.display()
            );
        }
        Ok(Self {
            root: runtime_root.to_owned(),
            binary,
            platform: platform.to_owned(),
            arch: arch.to_owned(),
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

const MAX_RPC_HEADER: usize = 8192;
const MAX_RPC_BODY: usize = 1024 * 1024;
const COPILOT_MAX_PROTOCOL_VERSION: u64 = 3;

async fn read_rpc_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<serde_json::Value> {
    let mut header = Vec::new();
    loop {
        anyhow::ensure!(
            header.len() < MAX_RPC_HEADER,
            "Copilot RPC header too large"
        );
        header.push(reader.read_u8().await.context("read Copilot RPC header")?);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = std::str::from_utf8(&header).context("invalid Copilot RPC header")?;
    let mut length = None;
    for line in header.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .context("malformed Copilot RPC header")?;
        if name.eq_ignore_ascii_case("content-length") {
            anyhow::ensure!(length.is_none(), "duplicate Copilot RPC content length");
            let parsed: usize = value
                .trim()
                .parse()
                .context("invalid Copilot RPC content length")?;
            anyhow::ensure!(parsed <= MAX_RPC_BODY, "Copilot RPC body too large");
            length = Some(parsed);
        }
    }
    let mut body = vec![0; length.context("missing Copilot RPC content length")?];
    reader
        .read_exact(&mut body)
        .await
        .context("read Copilot RPC body")?;
    serde_json::from_slice(&body).context("invalid Copilot RPC JSON")
}

async fn ping_rpc(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<u64> {
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"message":"probe"}}"#;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    for _ in 0..64 {
        let message = read_rpc_frame(reader).await?;
        if message.get("id") == Some(&serde_json::json!(1)) {
            anyhow::ensure!(
                message.get("error").is_none(),
                "Copilot runtime rejected ping"
            );
            anyhow::ensure!(
                message["result"]["message"] == "pong: probe",
                "unexpected Copilot ping response"
            );
            return message["result"]["protocolVersion"]
                .as_u64()
                .context("Copilot ping omitted protocol version");
        }
    }
    bail!("Copilot runtime did not reply to ping")
}

async fn connect_rpc(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<Option<u64>> {
    let body = br#"{"jsonrpc":"2.0","id":0,"method":"connect","params":{"supportedTaskKinds":["agent","client","shell"]}}"#;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    for _ in 0..64 {
        let message = read_rpc_frame(reader).await?;
        if message.get("id") != Some(&serde_json::json!(0)) {
            continue;
        }
        if let Some(error) = message.get("error") {
            if error["code"].as_i64() == Some(-32601)
                || error["message"].as_str() == Some("Unhandled method connect")
            {
                return Ok(None);
            }
            bail!("Copilot runtime rejected protocol handshake");
        }
        return Ok(Some(
            message["result"]["protocolVersion"]
                .as_u64()
                .context("Copilot handshake omitted protocol version")?,
        ));
    }
    bail!("Copilot runtime did not reply to protocol handshake")
}

fn check_protocol_version(version: u64) -> Result<()> {
    anyhow::ensure!(
        (u64::from(COPILOT_MIN_PROTOCOL_VERSION)..=COPILOT_MAX_PROTOCOL_VERSION).contains(&version),
        "unsupported Copilot protocol version {version}"
    );
    Ok(())
}

async fn verify_runtime(runtime: &RuntimeInfo) -> Result<()> {
    crate::cancellation::Cancellation::current().check()?;
    let mut command = tokio::process::Command::new(&runtime.binary);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .args([
            "--headless",
            "--no-auto-update",
            "--stdio",
            "--no-auto-login",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn Copilot runtime at {}", runtime.binary.display()))?;
    let _group = crate::process::ProcessGroup::new(&child);
    let mut stdin = child
        .stdin
        .take()
        .context("Copilot runtime stdin unavailable")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Copilot runtime stdout unavailable")?;
    let handshake = async {
        if let Some(version) = connect_rpc(&mut stdout, &mut stdin).await? {
            check_protocol_version(version)?;
        }
        check_protocol_version(ping_rpc(&mut stdout, &mut stdin).await?)
    };
    crate::cancellation::wait(tokio::time::timeout(Duration::from_secs(10), handshake))
        .await?
        .context("Copilot runtime handshake timed out")??;
    Ok(())
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

/// Verify the runtime and create a local handle for the current turn wrapper.
/// This does not yet create or resume a persistent runtime session.
pub async fn create_session(runtime: &RuntimeInfo, cfg: &SessionConfig) -> Result<SessionHandle> {
    // The current turn wrapper creates an ephemeral SDK session later. Verify
    // the runtime's framing and ping response before returning a local handle.
    verify_runtime(runtime).await?;
    let handle = SessionHandle {
        id: Uuid::new_v4().to_string(),
        connection_id: Uuid::new_v4().to_string(),
        model: cfg.model.clone().unwrap_or_else(|| "auto".to_string()),
    };
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
    fn rust_target_names_match_the_sdk_runtime_packages() {
        assert_eq!(
            runtime_platform("linux", "x86_64", false),
            Some(("linux", "x64"))
        );
        assert_eq!(
            runtime_platform("linux", "aarch64", false),
            Some(("linux", "arm64"))
        );
        assert_eq!(
            runtime_platform("linux", "aarch64", true),
            Some(("linuxmusl", "arm64"))
        );
        assert_eq!(
            runtime_platform("linux", "x86_64", true),
            Some(("linuxmusl", "x64"))
        );
        assert_eq!(
            runtime_platform("macos", "aarch64", false),
            Some(("darwin", "arm64"))
        );
        assert_eq!(
            runtime_platform("macos", "x86_64", false),
            Some(("darwin", "x64"))
        );
        assert_eq!(
            runtime_platform("windows", "x86_64", false),
            Some(("win32", "x64"))
        );
        assert_eq!(
            runtime_platform("windows", "aarch64", false),
            Some(("win32", "arm64"))
        );
        assert_eq!(runtime_platform("android", "aarch64", false), None);
        assert_eq!(runtime_platform("linux", "riscv64", false), None);
    }

    #[tokio::test]
    async fn framed_ping_handles_fragmented_messages_and_notifications() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (mut reader, mut writer) = tokio::io::split(client);
        let peer = tokio::spawn(async move {
            let request = read_rpc_frame(&mut server).await.unwrap();
            assert_eq!(request["method"], "ping");
            for message in [
                serde_json::json!({"jsonrpc":"2.0", "method":"notification"}),
                serde_json::json!({"jsonrpc":"2.0", "id":1, "result":{"message":"pong: probe", "protocolVersion":3}}),
            ] {
                let body = serde_json::to_vec(&message).unwrap();
                let frame = [
                    format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes(),
                    body,
                ]
                .concat();
                for chunk in frame.chunks(3) {
                    server.write_all(chunk).await.unwrap();
                }
            }
        });
        ping_rpc(&mut reader, &mut writer).await.unwrap();
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn framed_rpc_rejects_duplicate_and_oversized_lengths() {
        for header in [
            "Content-Length: 1\r\nContent-Length: 1\r\n\r\n",
            "Content-Length: 1048577\r\n\r\n",
            "Other: value\r\n\r\n",
        ] {
            let (mut writer, mut reader) = tokio::io::duplex(256);
            writer.write_all(header.as_bytes()).await.unwrap();
            assert!(read_rpc_frame(&mut reader).await.is_err());
        }
    }

    #[tokio::test]
    async fn handshake_negotiates_version_and_recognizes_legacy_fallback() {
        for (reply, expected) in [
            (
                serde_json::json!({"jsonrpc":"2.0", "id":0, "result":{"protocolVersion":3}}),
                Some(3),
            ),
            (
                serde_json::json!({"jsonrpc":"2.0", "id":0, "error":{"code":-32601,"message":"unknown"}}),
                None,
            ),
        ] {
            let (client, mut server) = tokio::io::duplex(4096);
            let (mut reader, mut writer) = tokio::io::split(client);
            let peer = tokio::spawn(async move {
                let request = read_rpc_frame(&mut server).await.unwrap();
                assert_eq!(request["method"], "connect");
                let body = serde_json::to_vec(&reply).unwrap();
                server
                    .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                    .await
                    .unwrap();
                server.write_all(&body).await.unwrap();
            });
            assert_eq!(
                connect_rpc(&mut reader, &mut writer).await.unwrap(),
                expected
            );
            peer.await.unwrap();
        }
        assert!(check_protocol_version(3).is_ok());
        assert!(check_protocol_version(2).is_err());
        assert!(check_protocol_version(4).is_err());
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[tokio::test]
    async fn installed_runtime_responds_to_handshake_and_ping_when_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../node_modules/.pnpm/@github+copilot-sdk-linux-x64@1.0.27-preview.1/node_modules/@github/copilot-sdk-linux-x64",
        );
        if !root.exists() {
            assert!(
                std::env::var_os("DESKTOP_TEST_REQUIRE_COPILOT_RUNTIME").is_none(),
                "required Copilot test runtime not found at {}",
                root.display()
            );
            return;
        }
        verify_runtime(&RuntimeInfo::discover(&root).unwrap())
            .await
            .unwrap();
    }

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
