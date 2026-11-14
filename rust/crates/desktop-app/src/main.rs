//! # github-desktop-rs
//!
//! The application shell. Built directly on `tao` (event loop, window, native
//! menu) and `wry` (WebView2 / WKWebView / WebKitGTK). No Tauri, no Electron,
//! no Node runtime, and no TypeScript in the backend: everything below the web
//! layer lives in the GUI-free `desktop-core` crate.
//!
//! ```text
//!  web (desktop://)  <--evaluate_script--  main-process events
//!        |                                      ^
//!        | window.ipc.postMessage               | Host::emit
//!        v                                      |
//!   Bridge (worker thread) ---> DesktopApp ---> state dispatcher
//!                                   |
//!                                   +--> Host::window --> tao UI thread
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod backend;
mod bridge;
mod host;
mod instance;
mod menu;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native_file_drop;
mod navigation;
mod protocol;
mod shutdown;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Theme, WindowBuilder},
};
use wry::WebViewBuilder;

use desktop_core::app::DesktopApp;
use desktop_core::ipc::channels::event;

use crate::actions::ActionEffect;
use crate::bridge::{Bridge, INIT_SCRIPT, js_literal};
use crate::host::{AppEvent, TaoHost, WindowInfo};

const APP_URL: &str = "desktop://localhost/index.html";

// Only injected for --smoke-test. Wait for the real React application and
// round-trip both shell and backend requests through the native IPC bridge.
const SMOKE_SCRIPT: &str = r#"
if (location.pathname === '/index.html') {
  window.addEventListener('DOMContentLoaded', async () => {
    try {
      const deadline = Date.now() + 25000;
      while (!document.querySelector('#desktop-app-chrome')) {
        if (Date.now() >= deadline) throw new Error('React application did not mount');
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      const info = await window.__desktop.invoke('desktop.get-app-info');
      if (info?.runtime !== 'tao + wry') throw new Error('Unexpected shell RPC result');
      const config = await window.__desktop.invoke('desktop.backend.git_info', {
        operation: { type: 'global_config_path' }
      });
      if (typeof config !== 'string' || !config) throw new Error('Unexpected backend RPC result');
      window.ipc.postMessage('desktop-smoke:ok');
    } catch (error) {
      console.error('desktop smoke failed', error);
      window.ipc.postMessage('desktop-smoke:error');
    }
  }, { once: true });
}
"#;

fn main() {
    // Git invokes the current application binary as a private credential
    // helper. Dispatch it before initializing the GUI or its event loop.
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("__credential-helper")) {
        let operation = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .unwrap_or_default();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .and_then(|runtime| {
                runtime
                    .block_on(desktop_backend::git_credential_helper(&operation))
                    .map_err(std::io::Error::other)
            });
        if let Err(error) = result {
            eprintln!("github-desktop-rs: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run() {
        eprintln!("github-desktop-rs: {error}");
        std::process::exit(1);
    }
}

/// How often the colour scheme is re-read. A change made while the window has
/// focus is seen at once; otherwise it can take up to this long to appear.
const THEME_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Wakes the colour-scheme watcher early, so a check runs when the window
/// regains focus instead of waiting for the next poll.
#[derive(Clone, Default)]
struct ThemeNudge(Arc<(Mutex<bool>, std::sync::Condvar)>);

impl ThemeNudge {
    fn nudge(&self) {
        let (flag, ready) = &*self.0;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        ready.notify_one();
    }

    /// Sleep for `interval`, or until nudged, whichever comes first.
    fn wait(&self, interval: std::time::Duration) {
        let (flag, ready) = &*self.0;
        let mut pending = flag.lock().unwrap_or_else(|e| e.into_inner());
        if !*pending {
            pending = ready
                .wait_timeout(pending, interval)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *pending = false;
    }
}

/// Everything that must stop when the window is going away, in order: refuse
/// new host work, end child processes and release waiting streams, then close
/// the admission lanes so queued requests get their `Shutdown` answer.
fn begin_shutdown(
    app: Arc<DesktopApp>,
    host: Arc<TaoHost>,
    bridge: Arc<Bridge>,
    proxy: tao::event_loop::EventLoopProxy<AppEvent>,
) {
    let error_proxy = proxy.clone();
    if let Err(error) = shutdown::start(
        std::time::Duration::from_secs(5),
        move |deadline| {
            bridge.stop_admission();
            host.shutdown();
            app.shutdown_io();
            bridge.shutdown();
            bridge.wait_for_idle(deadline)
        },
        move |drained| {
            let _ = proxy.send_event(AppEvent::ShutdownComplete { drained });
        },
    ) {
        tracing::error!(%error, "could not start bounded shutdown");
        let _ = error_proxy.send_event(AppEvent::ShutdownComplete { drained: false });
    }
}

#[cfg(feature = "packaged-ui")]
fn write_package_shutdown_result(quit: &std::path::Path, status: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Write};
    if quit.file_name().and_then(|name| name.to_str()) != Some("quit") {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid package control path",
        ));
    }
    let directory = quit
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "missing package control directory"))?;
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "package control directory is not a directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn geteuid() -> std::os::raw::c_uint;
        }
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { geteuid() } {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "package control directory is not private",
            ));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(directory.join("result"))?
        .write_all(status.as_bytes())
}

/// Confirm exits that never created a window or admitted bridge work.
fn finish_without_window() -> std::result::Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "packaged-ui")]
    if let Some(path) = std::env::var_os("DESKTOP_PACKAGE_QUIT_FILE") {
        write_package_shutdown_result(std::path::Path::new(&path), "drained")?;
    }
    Ok(())
}

fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,renderer=warn")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let smoke_test = args.iter().any(|a| a == "--smoke-test");

    if args.iter().any(|a| a == "--check-dev-host") {
        if !cfg!(debug_assertions) || cfg!(feature = "packaged-ui") {
            return Err("development launcher requires a debug host without packaged-ui".into());
        }
        println!("desktop-dev-host-ok");
        return finish_without_window();
    }

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!(
            "github-desktop-rs {} (tao + wry, no Tauri)",
            env!("CARGO_PKG_VERSION")
        );
        return finish_without_window();
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return finish_without_window();
    }
    if args
        .iter()
        .any(|a| a == "--self-test" || a == "--no-window")
    {
        self_test()?;
        return finish_without_window();
    }

    // A launch that only carries a URL or CLI action belongs to the window
    // that is already open: hand it over and exit.
    let instance_socket = desktop_core::AppPaths::detect()?
        .user_data
        .join("instance.sock");
    if args.iter().any(|arg| instance::is_forwardable(arg)) {
        if instance::forward(&instance_socket, &args) {
            return finish_without_window();
        }
        // An older or unrecognized listener must not receive OAuth secrets,
        // and a new window cannot complete another window's pending sign-in.
        #[cfg(target_os = "linux")]
        if !matches!(std::fs::symlink_metadata(&instance_socket), Err(ref error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            let message = format!(
                "GitHub Desktop could not safely send this link to the open window. Close all GitHub Desktop windows, restart the application, and try the action again.\n\nIf the problem persists, check this socket after closing every window. Remove it only if it is a leftover from GitHub Desktop:\n{}",
                instance_socket.display()
            );
            eprintln!("{message}");
            rfd::MessageDialog::new()
                .set_title("GitHub Desktop")
                .set_level(rfd::MessageLevel::Warning)
                .set_description(&message)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return finish_without_window();
        }
    }

    // -- tao event loop -----------------------------------------------------
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let info = Arc::new(Mutex::new(WindowInfo {
        dark: desktop_core::platform::should_use_dark_colors(),
        ..WindowInfo::default()
    }));
    let host = TaoHost::new(proxy.clone(), info.clone());
    let app = DesktopApp::new(host.clone())?;

    let window = WindowBuilder::new()
        .with_title("GitHub Desktop (Rust)")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(880.0, 600.0))
        .with_visible(true)
        .build(&event_loop)?;

    protocol::register();
    if let Err(error) = instance::listen(&instance_socket, {
        let app = app.clone();
        let host = host.clone();
        move |args| {
            app.handle_launch_args(&args);
            // Bring the window forward: the user is coming back from a browser.
            let _ = desktop_core::Host::window(
                &*host,
                desktop_core::ipc::protocol::WindowAction::Focus,
            );
        }
    }) {
        tracing::warn!(%error, "single-instance handoff is unavailable; sign-in callbacks will open a second window");
    }

    // -- native menu --------------------------------------------------------
    let menu_model = app.snapshot()?.get("appMenu").cloned().unwrap_or_default();
    let menu_model: desktop_core::models::AppMenu =
        serde_json::from_value(menu_model).unwrap_or_default();
    let mut native_menu = menu::install(proxy.clone(), &menu_model.items);
    if let Some(menu) = &native_menu {
        menu.attach(&window);
    }

    // -- webview + bridge ---------------------------------------------------
    let bridge = Bridge::new(app.clone(), proxy.clone())?;
    let mut builder = WebViewBuilder::new()
        .with_asynchronous_custom_protocol("desktop".into(), {
            let bridge = bridge.clone();
            move |_id, request, responder| bridge.handle_protocol(request, responder)
        })
        .with_ipc_handler({
            let bridge = bridge.clone();
            let proxy = proxy.clone();
            move |request| {
                if smoke_test {
                    let result = match request.body().as_str() {
                        "desktop-smoke:ok" => Some(true),
                        "desktop-smoke:error" => Some(false),
                        _ => None,
                    };
                    if let Some(result) = result {
                        let _ = proxy.send_event(AppEvent::SmokeResult(result));
                        return;
                    }
                }
                bridge.handle_ipc(request.body());
            }
        })
        .with_initialization_script(host_env_script())
        .with_initialization_script(INIT_SCRIPT)
        .with_navigation_handler(|url| {
            if navigation::internal(&url) {
                return true;
            }
            if navigation::external(&url) {
                desktop_core::platform::open_external(&url);
            }
            false
        })
        .with_new_window_req_handler(|url, _| {
            if navigation::external(&url) {
                desktop_core::platform::open_external(&url);
            }
            wry::NewWindowResponse::Deny
        })
        .with_download_started_handler(|_, _| false)
        .with_devtools(true)
        .with_url(APP_URL);

    // Wry replaces WebView2's HTML drag-and-drop handler on Windows, which
    // would break the renderer's internal commit and branch drag gestures.
    // Keep that platform's existing behavior until it has a safe native path.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let proxy = proxy.clone();
        let external_drag = std::cell::Cell::new(false);
        builder = builder.with_drag_drop_handler(move |event| match event {
            wry::DragDropEvent::Enter { paths, .. } => {
                let is_file_drop = !paths.is_empty();
                external_drag.set(is_file_drop);
                is_file_drop
            }
            wry::DragDropEvent::Over { .. } => external_drag.get(),
            wry::DragDropEvent::Leave => external_drag.replace(false),
            wry::DragDropEvent::Drop { paths, .. } => {
                let is_file_drop = external_drag.replace(false) || !paths.is_empty();
                if let Some(paths) = native_file_drop::paths(paths) {
                    let _ = proxy.send_event(AppEvent::NativeFileDrop(paths));
                }
                is_file_drop
            }
            _ => false,
        });
    }

    if smoke_test {
        builder = builder.with_initialization_script(SMOKE_SCRIPT);
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(40));
            let _ = proxy.send_event(AppEvent::SmokeResult(false));
        });
    }

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = builder.build(&window)?;

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or("tao window has no default GTK vbox")?;
        builder.build_gtk(vbox)?
    };

    // The development launcher asks the shell to close through the normal
    // shutdown path instead of terminating it while Git work may be active.
    // Release builds never inspect this control file.
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("DESKTOP_DEV_QUIT_FILE") {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let path = std::path::PathBuf::from(path);
            loop {
                if path.is_file() {
                    let _ = proxy.send_event(AppEvent::DevelopmentShutdown);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
    }

    // This control channel is opt-in and private to the package supervisor.
    #[cfg(feature = "packaged-ui")]
    if let Some(path) = std::env::var_os("DESKTOP_PACKAGE_QUIT_FILE") {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let path = std::path::PathBuf::from(path);
            loop {
                if path.is_file() {
                    let _ = proxy.send_event(AppEvent::PackagedShutdown);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
    }

    // Initial state / launch actions are queued in the host outbox and flushed
    // as user events once the loop starts, so nothing is lost to load timing.
    app.broadcast_initial_state();
    app.handle_launch_args(&args);

    tracing::info!(url = APP_URL, "webview ready; entering tao event loop");

    // Desktops expose no reliable change event everywhere, so the colour
    // scheme is re-read periodically, and at once when the window regains focus.
    let theme_nudge = ThemeNudge::default();
    {
        let info = info.clone();
        let app = app.clone();
        let nudge = theme_nudge.clone();
        std::thread::Builder::new()
            .name("desktop-theme".into())
            .spawn(move || {
                loop {
                    nudge.wait(THEME_POLL_INTERVAL);
                    let dark = desktop_core::platform::should_use_dark_colors();
                    let changed = {
                        let mut guard = info.lock().unwrap_or_else(|e| e.into_inner());
                        let changed = guard.dark != dark;
                        guard.dark = dark;
                        changed
                    };
                    if changed {
                        app.emit(
                            desktop_core::ipc::channels::event::NATIVE_THEME_UPDATED,
                            vec![],
                        );
                    }
                }
            })
            .expect("failed to start the theme watcher");
    }

    let app_loop = app.clone();
    let host_loop = host.clone();
    let bridge_loop = bridge.clone();
    let info_loop = info.clone();
    let theme_nudge_loop = theme_nudge.clone();
    let proxy_loop = proxy.clone();
    let mut shutting_down = false;
    let mut smoke_outcome = None;
    #[cfg(debug_assertions)]
    let mut development_shutdown = false;
    #[cfg(feature = "packaged-ui")]
    let package_control =
        std::env::var_os("DESKTOP_PACKAGE_QUIT_FILE").map(std::path::PathBuf::from);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(user) => match user {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                AppEvent::NativeFileDrop(paths) => {
                    if shutting_down { return; }
                    let script = format!(
                        "window.dispatchEvent(new CustomEvent('desktop-native-file-drop',{{detail:{}}}))",
                        js_literal(&json!(paths))
                    );
                    if let Err(error) = webview.evaluate_script(&script) {
                        tracing::warn!(%error, "failed to deliver native file drop");
                    }
                }
                #[cfg(debug_assertions)]
                AppEvent::DevelopmentShutdown => {
                    if !shutting_down {
                        app_loop.emit("will-quit", vec![]);
                        shutting_down = true;
                        development_shutdown = true;
                        begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                    }
                }
                #[cfg(feature = "packaged-ui")]
                AppEvent::PackagedShutdown => {
                    if !shutting_down {
                        app_loop.emit("will-quit", vec![]);
                        shutting_down = true;
                        begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                    }
                }
                AppEvent::SmokeResult(passed) => {
                    if shutting_down { return; }
                    shutting_down = true;
                    smoke_outcome = Some(passed);
                    begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                }
                AppEvent::ShutdownComplete { drained } => {
                    if !shutting_down { return; }
                    #[cfg(feature = "packaged-ui")]
                    if let Some(path) = &package_control {
                        let status = if drained { "drained" } else { "timed-out" };
                        eprintln!("desktop-package-shutdown:{status}");
                        if let Err(error) = write_package_shutdown_result(path, status) {
                            eprintln!("failed to report packaged shutdown: {error}");
                            std::process::exit(1);
                        }
                        if !drained { std::process::exit(1); }
                    }
                    #[cfg(debug_assertions)]
                    if development_shutdown {
                        eprintln!("desktop-dev-shutdown:{}", if drained { "drained" } else { "timed-out" });
                        if !drained { std::process::exit(1); }
                    }
                    if !drained {
                        tracing::warn!("shutdown deadline elapsed with work still active");
                    }
                    if let Some(passed) = smoke_outcome {
                    if passed && drained {
                        eprintln!(
                            "PASS: native webview mounted React and completed shell and backend RPC"
                        );
                    } else {
                        eprintln!("FAIL: native webview smoke test");
                    }
                    std::process::exit(if passed && drained { 0 } else { 1 });
                    }
                    *control_flow = ControlFlow::Exit;
                }
                AppEvent::Flush => {
                    // Clear the scheduled flag first: anything published while
                    // this batch is delivered queues its own flush.
                    host_loop.flush_started();
                    let pending = host_loop.take_batch();
                    if pending.is_empty() {
                        return;
                    }
                    let mut failed = Vec::new();
                    for (channel, args) in pending {
                        // The native menu mirrors the snapshot the renderer gets.
                        if channel == event::APP_MENU {
                            if let (Some(menu), Some(items)) = (
                                native_menu.as_mut(),
                                args.get(0).and_then(|menu| menu.get("items")),
                            ) {
                                if let Ok(items) = serde_json::from_value::<Vec<_>>(items.clone()) {
                                    menu.sync(&items);
                                }
                            }
                        }
                        let script = format!(
                            "window.__desktop && window.__desktop.emit({},{})",
                            js_literal(&json!(channel)),
                            js_literal(&args)
                        );
                        if webview.evaluate_script(&script).is_err() {
                            failed.push((channel, args));
                        }
                    }
                    // Keep draining a backlog, but never spin while the webview
                    // refuses scripts: a failed batch waits for the next event
                    // or for the renderer's ready signal.
                    let retry = failed.is_empty() && host_loop.has_pending();
                    host_loop.requeue(failed);
                    if retry {
                        host_loop.schedule_flush();
                    }
                }
                AppEvent::RpcResult { id, result } => {
                    let payload =
                        serde_json::to_value(&result).unwrap_or_else(|_| json!({ "ok": false }));
                    let script = format!(
                        "window.__desktop && window.__desktop.resolve({},{})",
                        id,
                        js_literal(&payload)
                    );
                    if let Err(error) = webview.evaluate_script(&script) {
                        tracing::warn!(%error, id, "failed to resolve renderer invoke");
                    }
                }
                AppEvent::RpcProgress { id, payload } => {
                    let script = format!(
                        "window.__desktop && window.__desktop.progress({},{})",
                        id,
                        js_literal(&payload)
                    );
                    if let Err(error) = webview.evaluate_script(&script) {
                        tracing::warn!(%error, id, "failed to deliver renderer progress");
                    }
                }
                AppEvent::WindowAction { action, mut reply } => {
                    if shutdown::reject_window_action(shutting_down, &mut reply) {
                        return;
                    }
                    let (value, effect) =
                        actions::apply(action, &window, &webview, &info_loop, &app_loop);
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(value));
                    }
                    if matches!(effect, ActionEffect::Exit) {
                        if !shutting_down {
                            shutting_down = true;
                            begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                        }
                    }
                }
                AppEvent::NativeMenu(id) => {
                    if shutting_down { return; }
                    use desktop_core::ipc::protocol::WindowAction;
                    use desktop_core::menu::MenuAction;

                    let run =
                        |action| actions::apply(action, &window, &webview, &info_loop, &app_loop);
                    match app_loop.menu_action(&id) {
                        Some(MenuAction::Emit(name)) => {
                            app_loop.emit(event::MENU_EVENT, vec![json!(name)])
                        }
                        Some(MenuAction::Quit) => {
                            if !shutting_down {
                                app_loop.emit("will-quit", vec![]);
                                shutting_down = true;
                                begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                            }
                        }
                        Some(MenuAction::Edit(command)) => actions::edit(&webview, command),
                        Some(MenuAction::Zoom(direction)) => {
                            let current = info_loop
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .zoom_factor;
                            let next = desktop_core::menu::next_zoom(current, direction);
                            run(WindowAction::SetZoomFactor(next));
                            app_loop.emit(event::ZOOM_FACTOR_CHANGED, vec![json!(next)]);
                        }
                        Some(MenuAction::ToggleFullScreen) => {
                            let fullscreen = window.fullscreen().is_none();
                            window.set_fullscreen(
                                fullscreen.then_some(tao::window::Fullscreen::Borderless(None)),
                            );
                        }
                        Some(MenuAction::Reload) => {
                            run(WindowAction::Reload);
                        }
                        Some(MenuAction::ToggleDevTools) => {
                            run(WindowAction::DevTools);
                        }
                        Some(MenuAction::OpenUrl(url)) => {
                            run(WindowAction::OpenExternal(url));
                        }
                        Some(MenuAction::ShowLogs) => {
                            let logs = app_loop.paths.log_dir();
                            let _ = std::fs::create_dir_all(&logs);
                            run(WindowAction::UnsafeOpenDirectory(
                                logs.to_string_lossy().into_owned(),
                            ));
                        }
                        None => tracing::warn!(%id, "click on an unknown menu item"),
                    }
                }
            },

            Event::NewEvents(StartCause::Init) => {
                window.set_focus();
            }

            Event::WindowEvent {
                event: window_event,
                ..
            } => match window_event {
                WindowEvent::CloseRequested => {
                    // Electron's AppWindow emitted `will-quit` first. Keep that
                    // contract so a renderer can veto / persist on shutdown.
                    if !shutting_down {
                        app_loop.emit("will-quit", vec![]);
                        if smoke_test { smoke_outcome = Some(false); }
                        shutting_down = true;
                        begin_shutdown(app_loop.clone(), host_loop.clone(), bridge_loop.clone(), proxy_loop.clone());
                    }
                }
                WindowEvent::Focused(focused) => {
                    if focused {
                        // The user may have changed the scheme while away.
                        theme_nudge_loop.nudge();
                    }
                    info_loop.lock().unwrap_or_else(|e| e.into_inner()).focused = focused;
                    app_loop.emit(if focused { event::FOCUS } else { event::BLUR }, vec![]);
                }
                WindowEvent::Resized(_) => {
                    let maximized = window.is_maximized();
                    let mut guard = info_loop.lock().unwrap_or_else(|e| e.into_inner());
                    if guard.maximized != maximized {
                        guard.maximized = maximized;
                        guard.window_state = if maximized {
                            desktop_core::models::WindowState::Maximized
                        } else {
                            desktop_core::models::WindowState::Normal
                        };
                        drop(guard);
                        app_loop.emit(
                            event::WINDOW_STATE_CHANGED,
                            vec![json!(if maximized { "maximized" } else { "normal" })],
                        );
                    }
                }
                WindowEvent::ThemeChanged(theme) => {
                    let dark = matches!(theme, Theme::Dark);
                    info_loop.lock().unwrap_or_else(|e| e.into_inner()).dark = dark;
                    app_loop.emit(event::NATIVE_THEME_UPDATED, vec![json!(dark)]);
                }
                _ => {}
            },

            _ => {}
        }
    });
}

/// Host facts the renderer used to read from Node's `os` and `process`.
/// Injected before any page script so the `os` shim answers synchronously.
fn host_env_script() -> String {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let env = json!({
        "platform": platform,
        "arch": arch,
        "homedir": home,
        "tmpdir": std::env::temp_dir().to_string_lossy(),
    });
    format!("window.__desktopEnv = Object.freeze({});", js_literal(&env))
}

fn print_help() {
    println!(
        "github-desktop-rs {} — GitHub Desktop on tao + wry (no Tauri, no Electron)\n\n\
         Options:\n  \
         --version            print the version\n  \
         --help               print this help\n  \
         --self-test          run headless IPC/domain smoke tests and exit\n  \
         --cli-open=<path>    open a local repository on launch\n  \
         --cli-clone=<url>    clone a URL on launch\n\n\
         An x-github-client:// link or OAuth callback URL is forwarded to the\n\
         running window as a url-action event.",
        env!("CARGO_PKG_VERSION")
    );
}

/// Headless smoke test used by CI and by `--self-test`. Exercises the full
/// invoke path with a `NullHost`, so no window or webview is required.
fn self_test() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use desktop_core::ipc::protocol::NullHost;

    let app = DesktopApp::new(Arc::new(NullHost))?;
    let cases: &[(&str, Vec<Value>)] = &[
        ("desktop.get-app-info", vec![]),
        ("desktop.get-state", vec![]),
        ("get-app-path", vec![]),
        ("get-app-architecture", vec![]),
        ("get-current-window-state", vec![]),
        ("is-window-maximized", vec![]),
        ("get-guid", vec![]),
        ("get-notifications-permission", vec![]),
        ("desktop.get-preference", vec![json!({ "key": "tabSize" })]),
    ];

    for (channel, args) in cases {
        match app.invoke(channel, args.clone()) {
            Ok(value) => println!("ok   {channel:<34} {}", compact(&value)),
            Err(error) => {
                println!("FAIL {channel:<34} {error}");
                return Err(format!("self-test failed on {channel}: {error}").into());
            }
        }
    }

    app.send("renderer-ready", vec![json!(0.0)])?;
    app.send("log", vec![json!("info"), json!("self-test complete")])?;
    app.shutdown_io();
    println!("\nself-test: all {} channels answered", cases.len());
    Ok(())
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 96 {
        format!("{}…", text.chars().take(96).collect::<String>())
    } else {
        text
    }
}

#[cfg(all(test, feature = "packaged-ui"))]
mod packaged_shutdown_tests {
    #[test]
    fn result_never_overwrites_an_existing_file() {
        let directory =
            std::env::temp_dir().join(format!("desktop-control-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let result = directory.join("result");
        std::fs::write(&result, "keep").unwrap();
        assert!(super::write_package_shutdown_result(&directory.join("quit"), "drained").is_err());
        assert_eq!(std::fs::read_to_string(&result).unwrap(), "keep");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
