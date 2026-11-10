//! The wry <-> Rust transport.
//!
//! Three pieces replace Electron's `nodeIntegration: true` renderer:
//!
//! * a `desktop://` custom protocol serving the web layer (no `file://`, no
//!   Node filesystem access from JavaScript),
//! * an initialization script installing an `ipcRenderer`-compatible shim,
//! * an IPC handler receiving `window.ipc.postMessage` frames.
//!
//! `invoke` is implemented as a correlated request/response over
//! `postMessage` + `evaluate_script` rather than `fetch`. That avoids the
//! platform-specific CORS origins of custom schemes on WebView2 / WKWebView,
//! and keeps long-running git/API work off the webview resource thread. The
//! `/__invoke` custom-protocol endpoint is still provided for headless smoke
//! tests and non-webview clients.

use std::borrow::Cow;
use std::sync::{mpsc, Arc, Mutex};

use serde::Deserialize;
use serde_json::Value;
use tao::event_loop::EventLoopProxy;
use wry::RequestAsyncResponder;

use desktop_core::app::DesktopApp;
use desktop_core::ipc::channels::{event, send as tx};
use desktop_core::ipc::protocol::{IpcMessage, RpcResult};

use include_dir::{include_dir, Dir};

use crate::host::{AppEvent, TaoHost};

static VITE_DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../../out/vite-dist");
const DEV_VITE_DIST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../out/vite-dist");


/// Injected before any page script runs. It defines `window.__desktop`,
/// `window.desktop` and an `ipcRenderer`-shaped object so the web frontend (or
/// a browser build of the legacy React renderer) needs no Electron globals.
pub const INIT_SCRIPT: &str = include_str!("../web/ipc-shim.js");

const JSON: &str = "application/json; charset=utf-8";

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Incoming {
    Rpc {
        #[serde(rename = "__rpc")]
        id: u64,
        channel: String,
        #[serde(default)]
        args: Vec<Value>,
    },
    Send {
        channel: String,
        #[serde(default)]
        args: Vec<Value>,
    },
}

pub struct Bridge {
    app: Arc<DesktopApp>,
    /// A single worker consumes all renderer messages. Keeping it off the
    /// webview callback thread means `Host::window` can safely block on the
    /// tao event loop when a modal dialog needs an answer.
    incoming: Mutex<mpsc::Sender<Incoming>>,
    proxy: Mutex<EventLoopProxy<AppEvent>>,
}

impl Bridge {
    pub fn new(
        app: Arc<DesktopApp>,
        proxy: EventLoopProxy<AppEvent>,
        host: Arc<TaoHost>,
    ) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel::<Incoming>();
        let worker_app = app.clone();
        let worker_proxy = proxy.clone();

        std::thread::Builder::new()
            .name("desktop-ipc".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        Incoming::Send { channel, args } => {
                            if let Err(error) = worker_app.send(&channel, args) {
                                tracing::warn!(%channel, %error, "simplex IPC failed");
                                worker_app.emit(
                                    event::ERROR,
                                    vec![serde_json::json!({
                                        "type": "ipc",
                                        "channel": channel,
                                        "message": error.to_string(),
                                    })],
                                );
                            }
                            if channel == tx::RENDERER_READY {
                                // The renderer now has listeners registered;
                                // flush anything emitted during startup.
                                let _ = worker_proxy.send_event(AppEvent::Flush);
                            }
                        }
                        Incoming::Rpc { id, channel, args } => {
                            let result = match worker_app.invoke(&channel, args) {
                                Ok(value) => RpcResult::ok(value),
                                Err(error) => RpcResult::err(error),
                            };
                            if worker_proxy
                                .send_event(AppEvent::RpcResult { id, result })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                host.shutdown();
            })
            .expect("failed to start the desktop IPC worker");

        Arc::new(Self {
            app,
            incoming: Mutex::new(sender),
            proxy: Mutex::new(proxy),
        })
    }

    fn proxy_handle(&self) -> EventLoopProxy<AppEvent> {
        self.proxy.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `window.ipc.postMessage` entry point.
    pub fn handle_ipc(&self, body: &str) {
        match serde_json::from_str::<Incoming>(body) {
            // Host services (git, filesystem, HTTP) can run for a long time
            // and share no state, so each gets its own thread instead of
            // queueing behind the single IPC worker.
            // Process stdin writes and kills must keep the renderer's order, so
            // they go through the queue; everything else gets its own thread.
            Ok(Incoming::Rpc { id, channel, args })
                if desktop_core::io::handles(&channel) && !is_ordered_io(&channel) =>
            {
                let app = self.app.clone();
                let proxy = self.proxy_handle();
                let spawned = std::thread::Builder::new().name("desktop-io".into()).spawn(move || {
                    let result = match desktop_core::io::invoke(&app, &channel, &args) {
                        Ok(value) => RpcResult::ok(value),
                        Err(error) => RpcResult::err(error),
                    };
                    let _ = proxy.send_event(AppEvent::RpcResult { id, result });
                });
                if let Err(error) = spawned {
                    tracing::error!(%error, "could not start a host-service thread");
                    let result = RpcResult::err(desktop_core::error::AppError::msg(
                        "the system could not start another thread",
                    ));
                    let _ = self.proxy_handle().send_event(AppEvent::RpcResult { id, result });
                }
            }
            Ok(message) => {
                let _ = self
                    .incoming
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(message);
            }
            Err(error) => tracing::warn!(%error, body = %truncate(body), "malformed IPC frame"),
        }
    }

    /// `desktop://` custom protocol handler. Static assets answer inline;
    /// `__invoke` is moved to a worker so git/network calls never block the
    /// webview's resource thread.
    pub fn handle_protocol(
        &self,
        request: http::Request<Vec<u8>>,
        responder: RequestAsyncResponder,
    ) {
        let path = percent_decode(request.uri().path().trim_start_matches('/'));

        if path == "__health" {
            let info = match self.app.invoke("desktop.get-app-info", vec![]) {
                Ok(value) => RpcResult::ok(value),
                Err(error) => RpcResult::err(error),
            };
            responder.respond(response(200, JSON, info.to_bytes()));
            return;
        }

        // Metadata for the embedded renderer resources, so the `fs` shim can
        // answer stat/readdir for paths under the renderer's virtual app
        // directory.
        if path == "__bundle" {
            let target = request
                .uri()
                .query()
                .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("path=")))
                .map(percent_decode)
                .unwrap_or_default();
            let entry = bundle_entry(&target, self.app.paths.copilot_dir.as_deref());
            responder.respond(response(200, JSON, entry.to_string().into_bytes()));
            return;
        }

        if path == "__invoke" {
            if request.method() != http::Method::POST {
                responder.respond(response(
                    405,
                    JSON,
                    RpcResult::err(desktop_core::error::AppError::msg(
                        "__invoke requires POST",
                    ))
                    .to_bytes(),
                ));
                return;
            }
            // A body when the webview delivers one, otherwise the `payload`
            // query parameter (WebKitGTK drops custom-protocol bodies).
            let query_payload = request.uri().query().and_then(|query| {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("payload="))
                    .map(|value| percent_decode(value).into_bytes())
            });
            let body = request.into_body();
            let body = if body.is_empty() { query_payload.unwrap_or_default() } else { body };
            let app = self.app.clone();
            std::thread::Builder::new()
                .name("desktop-invoke".into())
                .spawn(move || {
                    let result = match IpcMessage::parse(&body) {
                        Ok(message) => match app.invoke(&message.channel, message.args) {
                            Ok(value) => RpcResult::ok(value),
                            Err(error) => RpcResult::err(error),
                        },
                        Err(error) => RpcResult::err(error),
                    };
                    responder.respond(response(200, JSON, result.to_bytes()));
                })
                .expect("failed to start an invoke worker");
            return;
        }

        if path.contains("..") || path.contains('\\') || path.contains(':') {
            responder.respond(response(403, "text/plain", b"forbidden".to_vec()));
            return;
        }

        match asset(&path) {
            Some((mime, bytes)) => responder.respond(response(200, &mime, bytes)),
            None => responder.respond(response(
                404,
                "text/plain; charset=utf-8",
                format!("asset not found: {path}").into_bytes(),
            )),
        }
    }
}

fn response(
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) -> http::Response<Cow<'static, [u8]>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::CACHE_CONTROL, "no-store")
        .header(http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Cow::Owned(body))
        .expect("valid response")
}

/// Load an asset, preferring the on-disk `out/vite-dist` in debug builds so
/// `cargo run` picks up the latest `pnpm vite:build` / `pnpm vite:dev`
/// output without needing a `cargo` rebuild. The embedded `VITE_DIST` is
/// the release fallback (single-file binary).
///
/// This is the correct answer to the hash churn problem: Vite writes
/// `assets/index-[hash].js` and a new `index.html` every build, but cargo's
/// `include_dir!` snapshot does not track that. Serving from disk in dev
/// makes `cargo run -p desktop-app` always serve the current hash.
fn load_disk(path: &str) -> Option<Vec<u8>> {
    let full = std::path::Path::new(DEV_VITE_DIST).join(path);
    // Don't escape the bundle root even in dev.
    if !full.starts_with(DEV_VITE_DIST) {
        return None;
    }
    std::fs::read(&full).ok()
}

fn asset(path: &str) -> Option<(Cow<'static, str>, Vec<u8>)> {
    // The Vite build of the renderer: index.html, hashed bundles under
    // assets/ and the static resources the UI loads by URL.
    let path = if path.is_empty() { "index.html" } else { path };
    // Debug: always fresh from disk, so hashes never get stale.
    #[cfg(debug_assertions)]
    {
        if let Some(bytes) = load_disk(path) {
            return Some((mime_for(path).into(), bytes));
        }
    }
    #[cfg(not(debug_assertions))]
    {
        if std::env::var("DESKTOP_SERVE_FROM_DISK").is_ok() {
            if let Some(bytes) = load_disk(path) {
                return Some((mime_for(path).into(), bytes));
            }
        }
    }
    let file = VITE_DIST.get_file(path)?;
    Some((mime_for(path).into(), file.contents().to_vec()))
}

/// Decode `%XX` escapes in a request path. Invalid escapes are kept verbatim;
/// the traversal checks run on the decoded result.
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

/// Describe a resource: a file, a directory with its entries, or
/// `null` when nothing is present at `path`. In debug builds this
/// consults the on-disk `out/vite-dist` first, so a new hashed file
/// (`assets/index-[hash].js`) is discovered without a rebuild.
/// Whether an io operation must run on the IPC worker, in arrival order.
fn is_ordered_io(channel: &str) -> bool {
    channel
        .strip_prefix(desktop_core::io::PREFIX)
        .is_some_and(|op| desktop_core::io::PROCESS_OPERATIONS.contains(&op))
}

/// Describe a file under the Copilot SDK package. The renderer reads that
/// folder through the same bundle endpoint as the web build, but the files
/// live on disk next to the app, not in the embedded bundle.
fn copilot_entry(root: &std::path::Path, rest: &str) -> Value {
    if rest.split('/').any(|part| part == "..") {
        return Value::Null;
    }
    let target = if rest.is_empty() { root.to_path_buf() } else { root.join(rest) };
    match std::fs::metadata(&target) {
        Ok(meta) if meta.is_file() => serde_json::json!({ "kind": "file", "size": meta.len() }),
        Ok(meta) if meta.is_dir() => {
            let mut entries: Vec<Value> = std::fs::read_dir(&target)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let kind = entry.file_type().ok()?;
                    Some(serde_json::json!({
                        "name": entry.file_name().to_string_lossy(),
                        "isDirectory": kind.is_dir(),
                    }))
                })
                .collect();
            entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            serde_json::json!({ "kind": "directory", "entries": entries })
        }
        _ => Value::Null,
    }
}

fn bundle_entry(path: &str, copilot_dir: Option<&std::path::Path>) -> Value {
    let path = path.trim_matches('/');
    if path.contains("..") || path.contains('\\') || path.contains(':') {
        return Value::Null;
    }
    if path == "copilot" || path.starts_with("copilot/") {
        return match copilot_dir {
            Some(root) => copilot_entry(root, path.strip_prefix("copilot").unwrap_or("").trim_start_matches('/')),
            None => Value::Null,
        };
    }
    // Debug: disk is authoritative for the current hash.
    #[cfg(debug_assertions)]
    {
        let base = std::path::Path::new(DEV_VITE_DIST).join(path);
        if base.is_file() {
            if let Ok(meta) = std::fs::metadata(&base) {
                return serde_json::json!({ "kind": "file", "size": meta.len() });
            }
        }
        if base.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&base) {
                let mut out: Vec<Value> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        serde_json::json!({ "name": name, "isDirectory": is_dir })
                    })
                    .collect();
                out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                return serde_json::json!({ "kind": "directory", "entries": out });
            }
        }
        // If disk missed (file not built yet), fall through to embedded.
    }
    if let Some(file) = VITE_DIST.get_file(path) {
        return serde_json::json!({ "kind": "file", "size": file.contents().len() });
    }
    let dir = if path.is_empty() { Some(&VITE_DIST) } else { VITE_DIST.get_dir(path) };
    match dir {
        Some(dir) => {
            let name = |entry: &std::path::Path| {
                entry.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            };
            let mut entries: Vec<Value> = dir
                .dirs()
                .map(|d| serde_json::json!({ "name": name(d.path()), "isDirectory": true }))
                .chain(dir.files().map(|f| serde_json::json!({ "name": name(f.path()), "isDirectory": false })))
                .collect();
            entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            serde_json::json!({ "kind": "directory", "entries": entries })
        }
        None => Value::Null,
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => JSON,
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "md" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn truncate(value: &str) -> &str {
    let end = value.char_indices().nth(200).map(|(i, _)| i).unwrap_or(value.len());
    &value[..end]
}

/// Serialize a JSON value so it can be embedded in a JavaScript expression
/// passed to `WebView::evaluate_script`. `U+2028`/`U+2029` are valid JSON but
/// terminate a JS string literal, so they must be escaped.
pub fn js_literal(value: &Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "null".to_string())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_handles_escapes_and_malformed_input() {
        assert_eq!(percent_decode("static/logo-64x64%402x.png"), "static/logo-64x64@2x.png");
        assert_eq!(percent_decode("a%2e%2E/b"), "a../b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%41"), "A");
    }

    #[test]
    fn bundle_entries_describe_files_and_directories() {
        assert_eq!(bundle_entry("index.html", None)["kind"], "file");
        assert!(bundle_entry("index.html", None)["size"].as_u64().unwrap() > 0);

        let root = bundle_entry("", None);
        assert_eq!(root["kind"], "directory");
        let names: Vec<&str> = root["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"index.html") && names.contains(&"assets"));
        assert_eq!(bundle_entry("/assets/", None)["kind"], "directory");

        assert_eq!(bundle_entry("missing", None), Value::Null);
        assert_eq!(bundle_entry("../Cargo.toml", None), Value::Null);
    }

    #[test]
    fn copilot_paths_are_described_from_the_sdk_directory() {
        let root = std::env::temp_dir().join(format!("desktop-copilot-entry-{}", std::process::id()));
        std::fs::create_dir_all(root.join("prebuilds/linux-x64")).unwrap();
        std::fs::write(root.join("prebuilds/linux-x64/copilot-runtime"), b"runtime").unwrap();

        assert_eq!(bundle_entry("copilot/prebuilds/linux-x64/copilot-runtime", Some(&root))["kind"], "file");
        assert_eq!(bundle_entry("copilot/prebuilds", Some(&root))["kind"], "directory");
        assert_eq!(bundle_entry("copilot/prebuilds/linux-x64/missing", Some(&root)), Value::Null);
        assert_eq!(bundle_entry("copilot/../bridge.rs", Some(&root)), Value::Null);
        // Without an SDK install nothing under copilot/ exists.
        assert_eq!(bundle_entry("copilot/prebuilds", None), Value::Null);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn assets_resolve_with_a_content_type() {
        let (mime, bytes) = asset("").expect("index.html is embedded");
        assert_eq!(mime.as_ref(), "text/html; charset=utf-8");
        assert!(!bytes.is_empty());
        assert!(asset("does/not/exist.js").is_none());
        assert_eq!(mime_for("static/x.svg"), "image/svg+xml");
    }

    #[test]
    fn asset_prefers_fresh_disk_hash_in_dev() {
        // In debug the on-disk `out/vite-dist` is authoritative. Create a
        // fresh hashed name, verify `asset` returns it without rebuilding
        // the embedded snapshot from `include_dir!`.
        #[cfg(debug_assertions)]
        {
            let tmp = std::path::Path::new(DEV_VITE_DIST).join("assets/stale-check-0a1b2c.js");
            std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
            std::fs::write(&tmp, b"// fresh hash\n").unwrap();
            let got = asset("assets/stale-check-0a1b2c.js").expect("disk asset should be served");
            assert_eq!(got.1, b"// fresh hash\n".to_vec());
            assert_eq!(got.0.as_ref(), "text/javascript; charset=utf-8");
            std::fs::remove_file(&tmp).ok();
            assert!(asset("assets/stale-check-0a1b2c.js").is_none() || {
                // Embedded fallback would not have this file, disk was cleaned
                asset("assets/stale-check-0a1b2c.js").is_none()
            });
        }
    }
}
