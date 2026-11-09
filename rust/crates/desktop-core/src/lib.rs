//! # desktop-core
//!
//! The complete non-UI half of GitHub Desktop, ported from TypeScript to Rust.
//! Deliberately free of any GUI, windowing or webview dependency: the crate
//! talks to its host through the [`Host`] trait, implemented by the `tao` +
//! `wry` binary in `desktop-app`.
//!
//! ## Layout
//!
//! | Module | TypeScript origin |
//! |--------|-------------------|
//! | [`models`] | `app/src/models/**` (61 files) |
//! | [`git`] | `app/src/lib/git/**` (71 files) |
//! | [`storage`] | `app/src/lib/databases`, `local-storage`, `token-store` |
//! | [`api`] | `app/src/lib/api.ts`, `http.ts` |
//! | [`state`] | `app-store.ts`, `dispatcher.ts`, sub-stores |
//! | [`ipc`] | `ipc-shared.ts`, `ipc-renderer.ts`, `ipc-main.ts` |
//! | [`platform`] | `main-process/*`, `shell.ts`, `notifications.ts` |
//!
//! ## Why no Tauri
//!
//! This migration uses `tao` (event loop / windows / menus) and `wry`
//! (WebView2 / WKWebView / WebKitGTK) directly. There is no Tauri runtime, no
//! `tauri.conf.json`, no Tauri command macro and no Node/Electron process.
//! The web layer is plain HTML/CSS/JS served from a `desktop://` custom
//! protocol and talks to Rust through a small `ipcRenderer`-compatible shim.

pub mod api;
pub mod app;
pub mod error;
pub mod git;
pub mod io;
pub mod ipc;
pub mod models;
pub mod platform;
pub mod state;
pub mod storage;

pub use app::{DesktopApp, STATE_CHANGED};
pub use error::{AppError, GitError, GitErrorKind, Result, RpcError};
pub use ipc::protocol::Host;
pub use models::app_state::AppState;
pub use storage::{AppPaths, SecureStore, Storage};

/// `log` channel: renderer log lines are forwarded to the main-process logger
/// (`main.ts` used winston). We map them onto `tracing` levels.
pub fn log_renderer(level: &str, message: &str) {
    match level.to_ascii_lowercase().as_str() {
        "error" => tracing::error!(target: "renderer", "{message}"),
        "warn" | "warning" => tracing::warn!(target: "renderer", "{message}"),
        "debug" => tracing::debug!(target: "renderer", "{message}"),
        "trace" => tracing::trace!(target: "renderer", "{message}"),
        _ => tracing::info!(target: "renderer", "{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ipc_contract_covers_all_electron_channels() {
        assert_eq!(ipc::channels::invoke::ALL.len(), 28);
        assert_eq!(ipc::channels::send::ALL.len(), 31);
        assert_eq!(ipc::channels::event::ALL.len(), 20);
        assert_eq!(28 + 31 + 20, 79);
        for channel in ipc::channels::invoke::ALL {
            assert!(ipc::channels::is_known(channel), "{channel}");
        }
    }

    #[test]
    fn unknown_channel_is_rejected() {
        let app = DesktopApp::new(Arc::new(ipc::protocol::NullHost)).unwrap();
        let error = app.invoke("does-not-exist", vec![]).unwrap_err();
        assert!(matches!(error, AppError::UnknownChannel(_)));
    }

    #[test]
    fn state_snapshot_never_contains_tokens() {
        let app = DesktopApp::new(Arc::new(ipc::protocol::NullHost)).unwrap();
        let snapshot = app.snapshot().unwrap();
        assert!(snapshot.get("endpointTokens").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn get_state_and_app_info_work_without_a_window() {
        let app = DesktopApp::new(Arc::new(ipc::protocol::NullHost)).unwrap();
        let info = app.invoke("desktop.get-app-info", vec![]).unwrap();
        assert_eq!(info["runtime"], "tao + wry");
        let state = app.invoke("desktop.get-state", vec![]).unwrap();
        assert!(state.get("repositories").is_some());
    }

    #[test]
    fn base64_roundtrip() {
        let encoded = storage::base64_encode(b"token:abc/123+=");
        assert_eq!(storage::base64_decode(&encoded).unwrap(), b"token:abc/123+=");
    }

    #[test]
    fn url_helpers() {
        assert_eq!(
            api::endpoints::get_enterprise_api_url("acme.ghe.com"),
            "https://api.acme.ghe.com"
        );
        assert_eq!(
            api::endpoints::get_enterprise_api_url("github.acme.com"),
            "https://github.acme.com/api/v3"
        );
        assert_eq!(api::endpoints::get_html_url(api::endpoints::DOTCOM_API_ENDPOINT), "https://github.com");
    }

    #[test]
    fn git_error_classification() {
        assert_eq!(
            error::parse_git_error("", "Permission denied (publickey)."),
            GitErrorKind::SshAuthenticationFailed
        );
        assert_eq!(
            error::parse_git_error("", "fatal: not a git repository"),
            GitErrorKind::NotAGitRepository
        );
    }

    #[test]
    fn unified_diff_parser() {
        let raw = "@@ -1,2 +1,2 @@ fn main()\n-old\n+new\n context\n";
        let hunks = git::diff::parse_unified_diff(raw);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines.len(), 3);
        assert_eq!(hunks[0].lines[1].text, "new");
    }
}
