//! The `Host` implementation that connects the GUI-free `desktop-core` to the
//! tao event loop.
//!
//! `desktop-core` is deliberately unaware of windows and webviews. Everything
//! it needs from the OS travels through [`Host::emit`] (main-process pushes)
//! and [`Host::window`] (UI-thread operations). This module turns those into
//! `EventLoopProxy` messages so that they are always applied on the thread that
//! owns the `tao::Window` and `wry::WebView`.
//!
//! Two details matter for correctness:
//!
//! 1. `emit` never calls `evaluate_script` directly. It appends to an outbox
//!    and posts a cheap `Flush` user event. That way a state broadcast fired
//!    during startup (before the webview exists) is not lost, and every event
//!    is delivered in order on the UI thread.
//! 2. Queries are answered from a shared [`WindowInfo`] snapshot rather than
//!    round-tripping through the event loop. Only modal dialogs actually block
//!    on a reply, which keeps the UI thread free and avoids re-entrancy
//!    deadlocks when Rust is calling into `Host` from a worker thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use serde_json::{json, Value};
use tao::event_loop::EventLoopProxy;

use desktop_core::error::{AppError, Result};
use desktop_core::ipc::protocol::{Host, RpcResult, WindowAction, WindowQuery};
use desktop_core::models::WindowState;

/// Reply channel used by modal actions that must return a value to the caller.
pub type ActionReply = mpsc::SyncSender<Result<Value>>;

/// Everything that can be posted into the tao event loop from Rust workers.
pub enum AppEvent {
    /// Drain the host outbox into `WebView::evaluate_script`.
    Flush,
    /// Apply an OS/window operation on the UI thread.
    WindowAction {
        action: WindowAction,
        reply: Option<ActionReply>,
    },
    /// Resolve a pending renderer `invoke` promise.
    RpcResult { id: u64, result: RpcResult },
    /// A native (muda) menu item was clicked.
    NativeMenu(String),
}

/// Cached, thread-safe mirror of the parts of `tao::Window` that `desktop-core`
/// may query synchronously. Updated from `WindowEvent`s in `main.rs`.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub maximized: bool,
    pub focused: bool,
    pub window_state: WindowState,
    pub zoom_factor: f64,
    pub dark: bool,
}

impl Default for WindowInfo {
    fn default() -> Self {
        Self {
            maximized: false,
            focused: true,
            window_state: WindowState::Normal,
            zoom_factor: 1.0,
            dark: false,
        }
    }
}

/// Events delivered to the webview per `Flush`. A larger backlog is split
/// across flushes, so the UI thread can handle other user events in between.
const BATCH_EVENTS: usize = 64;
/// Serialized payload delivered per `Flush`.
const BATCH_BYTES: usize = 256 * 1024;

pub struct TaoHost {
    proxy: EventLoopProxy<AppEvent>,
    info: Arc<Mutex<WindowInfo>>,
    outbox: Mutex<Vec<(String, Value)>>,
    running: AtomicBool,
    /// At most one `Flush` is queued at a time. Cleared when a flush starts,
    /// so an event published during the flush queues the next one.
    flush_scheduled: AtomicBool,
}

impl TaoHost {
    pub fn new(proxy: EventLoopProxy<AppEvent>, info: Arc<Mutex<WindowInfo>>) -> Arc<Self> {
        Arc::new(Self {
            proxy,
            info,
            outbox: Mutex::new(Vec::new()),
            running: AtomicBool::new(true),
            flush_scheduled: AtomicBool::new(false),
        })
    }

    /// Post a `Flush` unless one is already queued.
    pub fn schedule_flush(&self) {
        if !self.flush_scheduled.swap(true, Ordering::AcqRel) {
            // A failed Flush means the loop is gone; there is nothing left to do.
            let _ = self.proxy.send_event(AppEvent::Flush);
        }
    }

    /// Called when a `Flush` starts. Events published from now on schedule
    /// their own flush instead of relying on the one that is running.
    pub fn flush_started(&self) {
        self.flush_scheduled.store(false, Ordering::Release);
    }

    /// Take the next batch, in insertion order, bounded by events and bytes.
    /// The outbox lock is released before the caller runs any JavaScript.
    pub fn take_batch(&self) -> Vec<(String, Value)> {
        let mut outbox = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
        let mut taken = 0usize;
        let mut bytes = 0usize;
        let mut count = 0usize;
        for (_, args) in outbox.iter() {
            if count == BATCH_EVENTS {
                break;
            }
            let size = args.to_string().len();
            // Always take at least one event, even an oversized one.
            if count > 0 && bytes + size > BATCH_BYTES {
                break;
            }
            bytes += size;
            count += 1;
            taken = count;
        }
        outbox.drain(..taken).collect()
    }

    pub fn has_pending(&self) -> bool {
        !self.outbox.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Re-queue events whose script evaluation failed (webview still loading).
    /// They go back to the front, so the order is kept.
    pub fn requeue(&self, mut events: Vec<(String, Value)>) {
        if events.is_empty() {
            return;
        }
        let mut outbox = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
        events.append(&mut *outbox);
        *outbox = events;
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn post(&self, event: AppEvent) -> Result<()> {
        self.proxy
            .send_event(event)
            .map_err(|_| AppError::msg("event loop has been closed"))
    }

    fn query(&self, query: WindowQuery) -> Value {
        let info = self
            .info
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match query {
            WindowQuery::IsMaximized => json!(info.maximized),
            WindowQuery::IsFocused => json!(info.focused),
            WindowQuery::WindowState => {
                serde_json::to_value(info.window_state).unwrap_or(Value::Null)
            }
            WindowQuery::ZoomFactor => json!(info.zoom_factor),
            WindowQuery::ShouldUseDarkColors => json!(info.dark),
            WindowQuery::IsInApplicationFolder => Value::Null,
        }
    }
}

impl Host for TaoHost {
    fn emit(&self, channel: &str, args: Value) {
        {
            let mut outbox = self.outbox.lock().unwrap_or_else(|e| e.into_inner());
            // Snapshots replace the one before them when nothing sits between
            // the two, so the renderer applies the latest state without
            // reordering any other event.
            let replaceable = channel == desktop_core::app::STATE_CHANGED
                && outbox.last().is_some_and(|(last, _)| last == channel);
            if replaceable {
                if let Some(last) = outbox.last_mut() {
                    last.1 = args;
                }
            } else {
                outbox.push((channel.to_string(), args));
            }
        }
        self.schedule_flush();
    }

    fn window(&self, action: WindowAction) -> Result<Value> {
        if !self.is_running() {
            return Ok(Value::Null);
        }

        // Queries are answered from the cached snapshot without touching the
        // event loop. This is what makes `is-window-maximized` cheap.
        if let WindowAction::Query(query) = action {
            return Ok(self.query(query));
        }

        // Modal dialogs and context menus must run on the UI thread and must
        // return a value. Block only for these four actions. All callers reach
        // this from a worker thread (see `Bridge::ipc_handler`), so the event
        // loop is free to answer.
        let needs_reply = matches!(
            action,
            WindowAction::ShowOpenDialog(_)
                | WindowAction::ShowSaveDialog(_)
                | WindowAction::ShowMessageBox(_)
                | WindowAction::ShowContextMenu(_)
        );

        if needs_reply {
            let (tx, rx) = mpsc::sync_channel(1);
            self.post(AppEvent::WindowAction {
                action,
                reply: Some(tx),
            })?;
            return rx
                .recv()
                .unwrap_or_else(|_| Err(AppError::msg("host did not answer the window action")));
        }

        self.post(AppEvent::WindowAction { action, reply: None })?;
        Ok(Value::Null)
    }
}
