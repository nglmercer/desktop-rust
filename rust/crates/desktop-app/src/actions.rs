//! Applies [`WindowAction`] on the tao UI thread.
//!
//! `desktop-core` formulates *what* should happen; this module is the only
//! place that is allowed to touch `tao::Window`, `wry::WebView`, `rfd`,
//! `arboard` or `muda`. Keeping that boundary explicit is what lets the core
//! crate stay GUI-free and unit-testable without a display server.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tao::window::{Theme, Window};
use wry::WebView;

use desktop_core::app::DesktopApp;
use desktop_core::error::{AppError, Result};
use desktop_core::ipc::protocol::{
    MessageOptions, OpenDialogOptions, SaveDialogOptions, SerializableMenuItem, WindowAction,
    WindowQuery,
};
use desktop_core::models::ThemeSource;
use desktop_core::platform;

use crate::host::WindowInfo;

pub enum ActionEffect {
    Nothing,
    Exit,
}

/// Returns the value that resolves the originating `invoke` (if any) plus an
/// optional instruction to the event loop.
pub fn apply(
    action: WindowAction,
    window: &Window,
    webview: &WebView,
    info: &Arc<Mutex<WindowInfo>>,
    app: &Arc<DesktopApp>,
) -> (Value, ActionEffect) {
    let result = apply_inner(action, window, webview, info, app);
    match result {
        Ok((value, effect)) => (value, effect),
        Err(error) => {
            tracing::error!(%error, "window action failed");
            app.emit(
                desktop_core::ipc::channels::event::ERROR,
                vec![json!({ "type": "window", "message": error.to_string() })],
            );
            (Value::Null, ActionEffect::Nothing)
        }
    }
}

fn apply_inner(
    action: WindowAction,
    window: &Window,
    webview: &WebView,
    info: &Arc<Mutex<WindowInfo>>,
    app: &Arc<DesktopApp>,
) -> Result<(Value, ActionEffect)> {
    let mut effect = ActionEffect::Nothing;

    let value = match action {
        WindowAction::Minimize => {
            window.set_minimized(true);
            info.lock().unwrap_or_else(|e| e.into_inner()).window_state =
                desktop_core::models::WindowState::Minimized;
            Value::Null
        }
        WindowAction::Maximize => {
            window.set_maximized(true);
            let mut guard = info.lock().unwrap_or_else(|e| e.into_inner());
            guard.maximized = true;
            guard.window_state = desktop_core::models::WindowState::Maximized;
            Value::Null
        }
        WindowAction::Unmaximize => {
            window.set_maximized(false);
            let mut guard = info.lock().unwrap_or_else(|e| e.into_inner());
            guard.maximized = false;
            guard.window_state = desktop_core::models::WindowState::Normal;
            Value::Null
        }
        WindowAction::Close | WindowAction::Quit | WindowAction::QuitAndInstallUpdate => {
            effect = ActionEffect::Exit;
            Value::Null
        }
        WindowAction::Focus => {
            window.set_focus();
            Value::Null
        }
        WindowAction::DevTools => {
            if webview.is_devtools_open() {
                webview.close_devtools();
            } else {
                webview.open_devtools();
            }
            Value::Null
        }
        WindowAction::Reload => {
            webview.reload().map_err(|e| AppError::msg(e.to_string()))?;
            Value::Null
        }
        WindowAction::SelectAll => {
            let _ = webview.evaluate_script("document.execCommand('selectAll')");
            Value::Null
        }
        WindowAction::SetZoomFactor(factor) => {
            let _ = webview.zoom(factor);
            info.lock().unwrap_or_else(|e| e.into_inner()).zoom_factor = factor;
            Value::Null
        }
        WindowAction::SetBackgroundColor(color) => {
            if let Some(rgba) = parse_color(&color) {
                window.set_background_color(Some(rgba));
                let _ = webview.set_background_color(rgba);
            }
            Value::Null
        }
        WindowAction::SetTheme(source) => {
            let theme = match source {
                ThemeSource::System => None,
                ThemeSource::Light => Some(Theme::Light),
                ThemeSource::Dark => Some(Theme::Dark),
            };
            window.set_theme(theme);
            Value::Null
        }
        WindowAction::SetTitle(title) => {
            window.set_title(&title);
            Value::Null
        }
        WindowAction::OpenExternal(target) => json!(platform::open_external(&target)),
        WindowAction::ShowItemInFolder(path) => json!(platform::show_item_in_folder(&path)),
        WindowAction::UnsafeOpenDirectory(path) => json!(platform::open_external(&path)),
        WindowAction::MoveToTrash(path) => {
            platform::move_to_trash(&path)?;
            Value::Null
        }
        WindowAction::WriteClipboardText(text) => {
            let result = arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text))
                .map_err(|e| AppError::msg(format!("clipboard error: {e}")));
            result?;
            Value::Null
        }
        WindowAction::ShowOpenDialog(options) => show_open_dialog(&options),
        WindowAction::ShowSaveDialog(options) => show_save_dialog(&options),
        WindowAction::ShowMessageBox(options) => show_message_box(&options),
        WindowAction::ShowContextMenu(items) => show_context_menu(&items, app),
        WindowAction::InstallCli(install) => {
            platform::install_cli(install, &app.paths.user_data, &app.paths.exec_path)?;
            Value::Null
        }
        WindowAction::Query(window_query) => query(window_query, info),
    };

    Ok((value, effect))
}

fn query(query: WindowQuery, info: &Arc<Mutex<WindowInfo>>) -> Value {
    let guard = info.lock().unwrap_or_else(|e| e.into_inner());
    match query {
        WindowQuery::IsMaximized => json!(guard.maximized),
        WindowQuery::IsFocused => json!(guard.focused),
        WindowQuery::WindowState => json!(guard.window_state),
        WindowQuery::ZoomFactor => json!(guard.zoom_factor),
        WindowQuery::ShouldUseDarkColors => json!(guard.dark),
        WindowQuery::IsInApplicationFolder => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Dialogs (rfd). Results have the shape the renderer's IPC contract expects
// (`app/src/lib/ipc-shared.ts`), not Electron's raw `dialog` results.
// ---------------------------------------------------------------------------

fn apply_filters(
    mut dialog: rfd::FileDialog,
    filters: &[desktop_core::ipc::protocol::FileFilter],
) -> rfd::FileDialog {
    for filter in filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(filter.name.as_str(), &extensions);
    }
    dialog
}

fn show_open_dialog(options: &OpenDialogOptions) -> Value {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = &options.title {
        dialog = dialog.set_title(title.clone());
    }
    if let Some(default_path) = &options.default_path {
        let path = Path::new(default_path);
        dialog = if path.is_dir() {
            dialog.set_directory(path)
        } else {
            if let Some(parent) = path.parent() {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = path.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy().into_owned());
            }
            dialog
        };
    }
    dialog = apply_filters(dialog, &options.filters);

    let open_directory = options
        .properties
        .iter()
        .any(|p| p == "openDirectory" || p == "open-directory");
    let multi = options
        .properties
        .iter()
        .any(|p| p == "multiSelections" || p == "multi-selections");

    let paths: Vec<String> = if open_directory {
        if multi {
            dialog
                .pick_folders()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        } else {
            dialog
                .pick_folder()
                .map(|p| vec![p.to_string_lossy().into_owned()])
                .unwrap_or_default()
        }
    } else if multi {
        dialog
            .pick_files()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    } else {
        dialog
            .pick_file()
            .map(|p| vec![p.to_string_lossy().into_owned()])
            .unwrap_or_default()
    };

    // The renderer's `show-open-dialog` contract (AppWindow.showOpenDialog):
    // the first selected path, or null when the dialog was dismissed.
    paths
        .into_iter()
        .next()
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn show_save_dialog(options: &SaveDialogOptions) -> Value {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = &options.title {
        dialog = dialog.set_title(title.clone());
    }
    if let Some(default_path) = &options.default_path {
        let path = Path::new(default_path);
        if let Some(parent) = path.parent() {
            if parent.is_dir() {
                dialog = dialog.set_directory(parent);
            }
        }
        if let Some(name) = path.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy().into_owned());
        }
    }
    dialog = apply_filters(dialog, &options.filters);

    let path = dialog.save_file().map(|p| p.to_string_lossy().into_owned());
    // `show-save-dialog` resolves with the chosen path or null.
    path.map(Value::String).unwrap_or(Value::Null)
}

fn show_message_box(options: &MessageOptions) -> Value {
    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

    let mut dialog = MessageDialog::new()
        .set_title("GitHub Desktop")
        .set_level(MessageLevel::Info)
        .set_description(format!(
            "{}{}",
            options.message,
            options
                .detail
                .as_ref()
                .map(|d| format!("\n\n{d}"))
                .unwrap_or_default()
        ));

    dialog = match options.buttons.len() {
        0 | 1 => dialog.set_buttons(MessageButtons::Ok),
        2 => dialog.set_buttons(MessageButtons::OkCancel),
        _ => dialog.set_buttons(MessageButtons::YesNoCancel),
    };

    let result = dialog.show();
    let index = match result {
        MessageDialogResult::Ok | MessageDialogResult::Yes => options.default_id as i64,
        MessageDialogResult::Cancel => options.cancel_id as i64,
        MessageDialogResult::No => 1,
        MessageDialogResult::Custom(label) => options
            .buttons
            .iter()
            .position(|button| button == &label)
            .map(|i| i as i64)
            .unwrap_or(options.cancel_id as i64),
    };
    json!(index)
}

/// Context menus are rendered by the web layer (`only frontend in web`).
/// Rust supplies the serialized menu model; the DOM owns the presentation.
fn show_context_menu(items: &[SerializableMenuItem], app: &Arc<DesktopApp>) -> Value {
    app.emit(
        "context-menu",
        vec![json!({ "items": items, "x": 0, "y": 0 })],
    );
    Value::Null
}

/// Accept `#rgb`, `#rrggbb`, `#rrggbbaa` and the `rgb(r,g,b)` form used by
/// `app/src/ui/lib/globals.ts` for window background updates.
fn parse_color(value: &str) -> Option<(u8, u8, u8, u8)> {
    let trimmed = value.trim();
    if let Some(inner) = trimmed
        .strip_prefix("rgb(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut parts = inner.split(',').map(|p| p.trim().parse::<u8>().ok());
        return Some((parts.next()??, parts.next()??, parts.next()??, 255));
    }
    let hex = trimmed.trim_start_matches('#');
    let parse = |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok();
    match hex.len() {
        3 => Some((parse(0..1)? * 17, parse(1..2)? * 17, parse(2..3)? * 17, 255)),
        6 => Some((parse(0..2)?, parse(2..4)?, parse(4..6)?, 255)),
        8 => Some((parse(0..2)?, parse(2..4)?, parse(4..6)?, parse(6..8)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color("#fff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_color("#1f6feb"), Some((31, 111, 235, 255)));
        assert_eq!(parse_color("rgb(10, 20, 30)"), Some((10, 20, 30, 255)));
        assert_eq!(parse_color("nope"), None);
    }
}

/// Run a text editing command in the web view, as Electron's Edit menu
/// roles did.
pub fn edit(webview: &WebView, command: desktop_core::menu::EditCommand) {
    use desktop_core::menu::EditCommand;

    // WebKitGTK only lets the embedder paste; script-initiated paste is
    // refused without a user gesture.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;

        let name = match command {
            EditCommand::Undo => "Undo",
            EditCommand::Redo => "Redo",
            EditCommand::Cut => "Cut",
            EditCommand::Copy => "Copy",
            EditCommand::Paste => "Paste",
        };
        webview.webview().execute_editing_command(name);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let name = match command {
            EditCommand::Undo => "undo",
            EditCommand::Redo => "redo",
            EditCommand::Cut => "cut",
            EditCommand::Copy => "copy",
            EditCommand::Paste => "paste",
        };
        let _ = webview.evaluate_script(&format!("document.execCommand('{name}')"));
    }
}
