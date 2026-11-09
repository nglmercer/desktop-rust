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
        })
    }

    /// `window.ipc.postMessage` entry point.
    pub fn handle_ipc(&self, body: &str) {
        match serde_json::from_str::<Incoming>(body) {
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
            let body = request.into_body();
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
            Some((mime, bytes)) => responder.respond(response(200, mime, bytes.to_vec())),
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

fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    // The Vite build of the renderer: index.html, hashed bundles under
    // assets/ and the static resources the UI loads by URL.
    let path = if path.is_empty() { "index.html" } else { path };
    let file = VITE_DIST.get_file(path)?;
    Some((mime_for(path), file.contents()))
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
    fn assets_resolve_with_a_content_type() {
        let (mime, bytes) = asset("").expect("index.html is embedded");
        assert_eq!(mime, "text/html; charset=utf-8");
        assert!(!bytes.is_empty());
        assert!(asset("does/not/exist.js").is_none());
        assert_eq!(mime_for("static/x.svg"), "image/svg+xml");
    }
}
