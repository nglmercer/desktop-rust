//! The application runtime. Owns all state in Rust and exposes the two IPC
//! entry points (`invoke` / `send`) to the tao+wry shell.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};

use crate::api::ApiClient;
use crate::error::{AppError, Result};
use crate::git::GitEngine;
use crate::ipc::protocol::{Host, IpcMessage};
use crate::models::app_state::AppState;
use crate::state::dispatcher;
use crate::storage::{AppPaths, SecureStore, Storage};

/// A new channel introduced by the Rust port. Electron's renderer read
/// `AppStore` state in-process; wry has no shared memory, so Rust pushes a
/// sanitized immutable snapshot on every mutation.
pub const STATE_CHANGED: &str = "state-changed";

pub struct DesktopApp {
    pub paths: AppPaths,
    pub storage: Storage,
    pub secure: SecureStore,
    pub git: GitEngine,
    pub api: ApiClient,
    state: Mutex<AppState>,
    menu_labels: Mutex<crate::menu::MenuLabels>,
    host: Arc<dyn Host>,
    started: Instant,
}

impl DesktopApp {
    pub fn new(host: Arc<dyn Host>) -> Result<Arc<Self>> {
        let paths = AppPaths::detect()?;
        let storage = Storage::open(paths.clone())?;
        let secure = SecureStore::new(paths.credential_dir())?;
        let api = ApiClient::new(secure.clone())?;
        let git = GitEngine::new();

        let mut state = storage.load_state()?;
        if let Ok(prefs) = std::fs::read(paths.preferences_file()) {
            if let Ok(prefs) = serde_json::from_slice(&prefs) {
                state.preferences = prefs;
            }
        }
        state.accounts = state.accounts.iter().map(|a| a.without_token()).collect();
        state.endpoint_tokens.clear();
        // The menu is rebuilt on every start; a persisted copy may predate it.
        state.app_menu.items = crate::menu::build(&crate::menu::MenuLabels::default());

        tracing::info!(
            git = %git.binary.to_string_lossy(),
            data = %paths.user_data.display(),
            repositories = state.repositories.len(),
            "desktop-core initialized"
        );

        Ok(Arc::new(Self {
            paths,
            storage,
            secure,
            git,
            api,
            state: Mutex::new(state),
            menu_labels: Mutex::new(crate::menu::MenuLabels::default()),
            host,
            started: Instant::now(),
        }))
    }

    pub fn started(&self) -> Instant { self.started }

    /// The host, for services that emit events from their own threads.
    pub fn host(&self) -> Arc<dyn Host> {
        self.host.clone()
    }

    /// The renderer state the menu wording currently reflects.
    pub fn menu_labels(&self) -> crate::menu::MenuLabels {
        self.menu_labels.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_menu_labels(&self, labels: crate::menu::MenuLabels) {
        *self.menu_labels.lock().unwrap_or_else(|e| e.into_inner()) = labels;
    }

    /// What clicking the menu item `id` does.
    pub fn menu_action(&self, id: &str) -> Option<crate::menu::MenuAction> {
        crate::menu::action(id, &self.menu_labels())
    }

    /// Duplex IPC (`ipcRenderer.invoke`). Returns the value that resolves the
    /// JS promise, or a flattened error.
    pub fn invoke(&self, channel: &str, args: Vec<Value>) -> Result<Value> {
        // Host services are stateless; keep them off the state lock so a
        // long git or HTTP call cannot stall every other command.
        if crate::io::handles(channel) {
            return crate::io::invoke(self, channel, &args);
        }
        let message = IpcMessage { channel: channel.to_string(), args };
        let outcome = {
            let mut state = self.lock_state()?;
            dispatcher::invoke(self, &mut state, &message)
        };
        let (value, changed) = outcome?;
        if changed {
            self.persist_and_broadcast()?;
        }
        Ok(value)
    }

    /// Simplex IPC (`ipcRenderer.send`). Fire-and-forget, but may mutate state.
    pub fn send(&self, channel: &str, args: Vec<Value>) -> Result<()> {
        let message = IpcMessage { channel: channel.to_string(), args };
        let changed = {
            let mut state = self.lock_state()?;
            dispatcher::send(self, &mut state, &message)
        }?;
        if changed {
            self.persist_and_broadcast()?;
        }
        Ok(())
    }

    /// `ipcWebContents.send` — push an event into the webview. `args` is the
    /// array of positional arguments; the JS shim spreads it back out.
    pub fn emit(&self, channel: &str, args: Vec<Value>) {
        self.host.emit(channel, Value::Array(args));
    }

    pub fn emit_value(&self, channel: &str, value: Value) {
        self.emit(channel, vec![value]);
    }

    pub fn window(&self, action: crate::ipc::protocol::WindowAction) -> Result<Value> {
        self.host.window(action)
    }

    pub fn snapshot(&self) -> Result<Value> {
        let state = self.lock_state()?;
        Ok(serde_json::to_value(state.sanitized())?)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, AppState>> {
        self.state.lock().map_err(|_| AppError::msg("app state lock poisoned"))
    }

    fn persist_and_broadcast(&self) -> Result<()> {
        let (state, preferences) = {
            let guard = self.lock_state()?;
            (guard.sanitized(), guard.preferences.clone())
        };
        self.storage.save_state(&state)?;
        self.storage.save_preferences(&preferences)?;
        self.emit_value(STATE_CHANGED, json!(state));
        Ok(())
    }

    /// Called once the webview has loaded so the renderer can hydrate.
    pub fn broadcast_initial_state(&self) {
        match self.snapshot() {
            Ok(state) => {
                let menu = state.get("appMenu").cloned().unwrap_or(json!({}));
                self.emit_value(STATE_CHANGED, state);
                self.emit(crate::ipc::channels::event::APP_MENU, vec![menu]);
                self.emit(
                    crate::ipc::channels::event::LAUNCH_TIMING_STATS,
                    vec![json!({
                        "mainReadyTime": self.started.elapsed().as_millis() as f64,
                        "loadTime": 0.0,
                        "rendererReadyTime": 0.0
                    })],
                );
            }
            Err(e) => tracing::error!("failed to serialize initial state: {e}"),
        }
    }

    /// Route OS-level launch arguments (`--cli-open`, `--cli-clone`, and the
    /// `x-github-client://` protocol URLs) into `cli-action` / `url-action`.
    pub fn handle_launch_args(&self, args: &[String]) {
        for arg in args {
            if let Some(path) = arg.strip_prefix("--cli-open=") {
                self.emit(
                    crate::ipc::channels::event::CLI_ACTION,
                    vec![json!({ "kind": "open-repository", "path": path })],
                );
            } else if let Some(url) = arg.strip_prefix("--cli-clone=") {
                self.emit(
                    crate::ipc::channels::event::CLI_ACTION,
                    vec![json!({ "kind": "clone-url", "url": url })],
                );
            } else if is_app_url(arg) {
                self.emit(crate::ipc::channels::event::URL_ACTION, vec![parse_app_url(arg)]);
            }
        }
    }
}

/// URL schemes that launch the app. `x-github-client` carries "open in
/// Desktop" links; the `*-auth` schemes carry the OAuth callback. Which auth
/// scheme GitHub redirects to is fixed by the OAuth app's registration, not
/// by this code: the development client id used by default redirects to
/// `x-github-desktop-dev-auth`.
pub const URL_SCHEMES: &[&str] = &[
    "x-github-client",
    "x-github-desktop-auth",
    "x-github-desktop-dev-auth",
    "github-mac",
    "github-windows",
];

pub fn is_app_url(arg: &str) -> bool {
    arg.split_once("://")
        .is_some_and(|(scheme, _)| URL_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()))
}

/// Port of `app/src/lib/parse-app-url.ts`. Produces a `URLActionType`.
pub fn parse_app_url(url: &str) -> Value {
    let unknown = || json!({ "name": "unknown", "url": url });

    let Some((_, rest)) = url.split_once("://") else {
        return unknown();
    };
    let rest = rest.split('#').next().unwrap_or("");
    let (location, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = match location.find('/') {
        Some(index) => (&location[..index], &location[index..]),
        None => (location, ""),
    };
    if host.is_empty() {
        return unknown();
    }

    // First occurrence wins, like `getQueryStringValue`.
    let param = |name: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
            .find(|(key, _)| percent_decode(key) == name)
            .map(|(_, value)| percent_decode(&value.replace('+', " ")))
    };

    match host.to_ascii_lowercase().as_str() {
        "oauth" => match (param("code"), param("state")) {
            (Some(code), Some(state)) => json!({ "name": "oauth", "code": code, "state": state }),
            _ => unknown(),
        },
        "openrepo" => {
            if path.len() <= 1 {
                return unknown();
            }
            let (pr, branch, filepath) = (param("pr"), param("branch"), param("filepath"));
            if let Some(pr) = &pr {
                if pr.is_empty() || !pr.bytes().all(|b| b.is_ascii_digit()) {
                    return unknown();
                }
                // A forked PR's branch must use the `pr/<number>` ref format.
                if let Some(branch) = &branch {
                    let valid = branch
                        .strip_prefix("pr/")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                    if !valid {
                        return unknown();
                    }
                }
            }
            if branch.as_deref().is_some_and(has_invalid_ref_chars) {
                return unknown();
            }
            json!({
                "name": "open-repository-from-url",
                // The legacy parser leaves the path as written.
                "url": &path[1..],
                "branch": branch,
                "pr": pr,
                "filepath": filepath,
            })
        }
        _ => unknown(),
    }
}

/// `testForInvalidChars` in `sanitize-ref-name.ts`: what git forbids in a
/// ref name, see https://git-scm.com/docs/git-check-ref-format
fn has_invalid_ref_chars(value: &str) -> bool {
    value.chars().any(|c| {
        (c as u32) <= 0x20
            || matches!(c, '\x7f' | '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '|' | '"' | '<' | '>')
    }) || value.contains("@{")
        || value.contains("..")
        || value.starts_with('.')
        || value.ends_with('.')
        || value.ends_with(".lock")
        || value.ends_with('/')
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_callback_is_parsed_for_every_auth_scheme() {
        for scheme in ["x-github-desktop-dev-auth", "x-github-desktop-auth", "x-github-client"] {
            let url = format!("{scheme}://oauth?code=abc%2F1&state=2f6c-uuid");
            assert!(is_app_url(&url));
            assert_eq!(
                parse_app_url(&url),
                json!({ "name": "oauth", "code": "abc/1", "state": "2f6c-uuid" })
            );
        }
        assert_eq!(parse_app_url("x-github-client://oauth?code=abc")["name"], "unknown");
    }

    #[test]
    fn open_repository_urls_are_validated() {
        let action = parse_app_url(
            "x-github-client://openRepo/https://github.com/desktop/desktop?branch=pr%2F12&pr=12",
        );
        assert_eq!(action["name"], "open-repository-from-url");
        assert_eq!(action["url"], "https://github.com/desktop/desktop");
        assert_eq!(action["branch"], "pr/12");
        assert_eq!(action["pr"], "12");
        assert_eq!(action["filepath"], Value::Null);

        for bad in [
            "x-github-client://openRepo/",
            "x-github-client://openRepo/https://github.com/a/b?pr=abc",
            "x-github-client://openRepo/https://github.com/a/b?pr=1&branch=main",
            "x-github-client://openRepo/https://github.com/a/b?branch=a..b",
            "x-github-client://openRepo/https://github.com/a/b?branch=a%20b",
            "x-github-client://openRepo/https://github.com/a/b?branch=x.lock",
            "x-github-client://openRepo/https://github.com/a/b?branch=a@{b",
            "x-github-client://somethingelse/x",
        ] {
            assert_eq!(parse_app_url(bad)["name"], "unknown", "{bad}");
        }
    }

    #[test]
    fn only_registered_schemes_are_app_urls() {
        assert!(!is_app_url("https://github.com/desktop/desktop"));
        assert!(!is_app_url("--cli-open=/tmp"));
        assert!(!is_app_url("/home/user/x-github-client://oauth"));
        assert!(is_app_url("X-GitHub-Client://openRepo/x"));
    }
}
