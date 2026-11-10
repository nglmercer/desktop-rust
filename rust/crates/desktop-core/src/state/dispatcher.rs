//! The Rust replacement for `app/src/main-process/main.ts`'s 59 `ipcMain`
//! handlers plus `AppWindow`. Every channel from `ipc-shared.ts` is routed
//! here; domain commands added by the port live in [`super::commands`].

use serde_json::{Value, json};

use crate::app::DesktopApp;
use crate::error::{AppError, Result};
use crate::ipc::channels::{event, invoke as ch, send as tx};
use crate::ipc::protocol::{
    IpcMessage, MenuStateUpdate, MessageOptions, OpenDialogOptions, SaveDialogOptions,
    SerializableMenuItem, WindowAction, WindowQuery,
};
use crate::models::app_state::AppState;
use crate::models::misc::{
    AppleActionOnDoubleClick, Architecture, DesktopNotificationPermission, ThemeSource,
};
use crate::platform;

use super::commands;

pub type Outcome = (Value, bool);

fn win(app: &DesktopApp, action: WindowAction) -> Result<Value> {
    app.window(action)
}

/// `ipcRenderer.invoke` / `ipcMain.handle` — 28 duplex channels.
pub fn invoke(app: &DesktopApp, state: &mut AppState, msg: &IpcMessage) -> Result<Outcome> {
    let channel = msg.channel.as_str();

    // Channels added by the Rust port (git, repositories, preferences, API…).
    if channel.starts_with("desktop.") {
        return commands::invoke(app, state, msg);
    }

    let value = match channel {
        ch::GET_PATH => json!(app.paths.get_path_by_name(&msg.string(0)?)),
        ch::GET_APP_ARCHITECTURE => serde_json::to_value(Architecture::current())?,
        // Electron's `app.getAppPath()` was the directory the renderer loaded
        // its resources from, i.e. its `__dirname`. The web build's resources
        // are served from the page origin under this virtual root instead.
        ch::GET_APP_PATH => json!(crate::io::BUNDLE_ROOT),
        ch::GET_EXEC_PATH => json!(app.paths.exec_path.to_string_lossy()),
        ch::IS_RUNNING_UNDER_ARM64_TRANSLATION => {
            json!(platform::is_running_under_arm64_translation())
        }
        ch::MOVE_TO_TRASH => {
            platform::move_to_trash(&msg.string(0)?)?;
            Value::Null
        }
        ch::SHOW_ITEM_IN_FOLDER => {
            json!(platform::show_item_in_folder(&msg.string(0)?))
        }
        ch::WRITE_CLIPBOARD_TEXT => win(app, WindowAction::WriteClipboardText(msg.string(0)?))?,
        ch::CONFIRM_REVEAL_DIRECTORY => {
            let response = win(
                app,
                WindowAction::ShowMessageBox(MessageOptions {
                    message: "Reveal Repository in Finder?".into(),
                    detail: Some(
                        "This will open the repository folder in your file manager.".into(),
                    ),
                    buttons: vec!["Reveal".into(), "Cancel".into()],
                    default_id: 0,
                    cancel_id: 1,
                }),
            )?;
            json!(response.as_i64().unwrap_or(1) == 0)
        }
        ch::SHOW_CONTEXTUAL_MENU => {
            let items: Vec<SerializableMenuItem> =
                serde_json::from_value(msg.arg(0)?.clone()).unwrap_or_default();
            win(app, WindowAction::ShowContextMenu(items))?
        }
        ch::IS_WINDOW_FOCUSED => win(app, WindowAction::Query(WindowQuery::IsFocused))?,
        ch::OPEN_EXTERNAL => {
            let target = msg.string(0)?;
            win(app, WindowAction::OpenExternal(target))?
        }
        ch::IS_IN_APPLICATION_FOLDER => json!(platform::is_in_application_folder()),
        ch::MOVE_TO_APPLICATIONS_FOLDER => {
            platform::move_to_applications_folder()?;
            Value::Null
        }
        ch::CHECK_FOR_UPDATES => {
            app.emit(event::AUTO_UPDATER_CHECKING_FOR_UPDATE, vec![]);
            match platform::check_for_updates(&msg.optional_string(0).unwrap_or_default()) {
                Ok(()) => {
                    app.emit(event::AUTO_UPDATER_UPDATE_NOT_AVAILABLE, vec![]);
                    Value::Null
                }
                Err(e) => {
                    app.emit(event::AUTO_UPDATER_ERROR, vec![json!(e.to_string())]);
                    return Err(e);
                }
            }
        }
        ch::GET_CURRENT_WINDOW_STATE => win(app, WindowAction::Query(WindowQuery::WindowState))?,
        ch::GET_CURRENT_WINDOW_ZOOM_FACTOR => json!(state.window_zoom_factor),
        ch::RESOLVE_PROXY => json!(platform::resolve_proxy(&msg.string(0)?)),
        ch::SHOW_SAVE_DIALOG => {
            let options: SaveDialogOptions =
                serde_json::from_value(msg.arg(0).cloned().unwrap_or(json!({})))?;
            win(app, WindowAction::ShowSaveDialog(options))?
        }
        ch::SHOW_OPEN_DIALOG => {
            let options: OpenDialogOptions =
                serde_json::from_value(msg.arg(0).cloned().unwrap_or(json!({})))?;
            win(app, WindowAction::ShowOpenDialog(options))?
        }
        ch::IS_WINDOW_MAXIMIZED => win(app, WindowAction::Query(WindowQuery::IsMaximized))?,
        ch::GET_APPLE_ACTION_ON_DOUBLE_CLICK => {
            serde_json::to_value(AppleActionOnDoubleClick::Maximize)?
        }
        ch::SHOULD_USE_DARK_COLORS => json!(platform::should_use_dark_colors()),
        ch::GET_GUID => json!(app.storage.get_guid()?),
        ch::SAVE_GUID => {
            app.storage.save_guid(&msg.string(0)?)?;
            Value::Null
        }
        ch::SHOW_NOTIFICATION => {
            let title = msg.string(0)?;
            let body = msg.optional_string(1).unwrap_or_default();
            let user_info = msg.arg(2).cloned().unwrap_or(json!({}));
            let id = platform::show_notification(&title, &body, &user_info);
            app.emit(
                event::NOTIFICATION_EVENT,
                vec![json!("click"), json!(id), user_info],
            );
            json!(id)
        }
        ch::GET_NOTIFICATIONS_PERMISSION => {
            serde_json::to_value(platform::notifications_permission())?
        }
        ch::REQUEST_NOTIFICATIONS_PERMISSION => {
            json!(platform::request_notifications_permission())
        }
        _ => return Err(AppError::UnknownChannel(channel.to_string())),
    };
    Ok((value, false))
}

/// `ipcRenderer.send` / `ipcMain.on` — 31 inbound simplex channels.
/// Returns `true` when app state changed and should be persisted/broadcast.
pub fn send(app: &DesktopApp, state: &mut AppState, msg: &IpcMessage) -> Result<bool> {
    let channel = msg.channel.as_str();

    if channel.starts_with("desktop.") {
        let (_, changed) = commands::invoke(app, state, msg)?;
        return Ok(changed);
    }

    match channel {
        tx::MINIMIZE_WINDOW => {
            win(app, WindowAction::Minimize)?;
        }
        tx::MAXIMIZE_WINDOW => {
            win(app, WindowAction::Maximize)?;
        }
        tx::UNMAXIMIZE_WINDOW => {
            win(app, WindowAction::Unmaximize)?;
        }
        tx::CLOSE_WINDOW => {
            win(app, WindowAction::Close)?;
        }
        tx::FOCUS_WINDOW => {
            win(app, WindowAction::Focus)?;
        }
        tx::QUIT_APP => {
            win(app, WindowAction::Quit)?;
        }
        tx::QUIT_AND_INSTALL_UPDATES => {
            win(app, WindowAction::QuitAndInstallUpdate)?;
        }
        tx::SELECT_ALL_WINDOW_CONTENTS => {
            win(app, WindowAction::SelectAll)?;
        }
        tx::SET_WINDOW_ZOOM_FACTOR => {
            let factor = msg.arg(0)?.as_f64().unwrap_or(1.0);
            state.window_zoom_factor = factor;
            win(app, WindowAction::SetZoomFactor(factor))?;
            app.emit(event::ZOOM_FACTOR_CHANGED, vec![json!(factor)]);
            return Ok(true);
        }
        tx::UPDATE_WINDOW_BACKGROUND_COLOR => {
            if let Some(color) = msg.optional_string(0) {
                win(app, WindowAction::SetBackgroundColor(color))?;
            }
        }
        tx::SET_NATIVE_THEME_SOURCE => {
            let source: ThemeSource =
                serde_json::from_value(msg.arg(0).cloned().unwrap_or(json!("system")))
                    .unwrap_or_default();
            state.preferences.theme_source = source;
            win(app, WindowAction::SetTheme(source))?;
            app.emit(event::NATIVE_THEME_UPDATED, vec![]);
            return Ok(true);
        }
        tx::INSTALL_WINDOWS_CLI => {
            win(app, WindowAction::InstallCli(true))?;
        }
        tx::UNINSTALL_WINDOWS_CLI => {
            win(app, WindowAction::InstallCli(false))?;
        }
        tx::UNSAFE_OPEN_DIRECTORY => {
            win(app, WindowAction::UnsafeOpenDirectory(msg.string(0)?))?;
        }
        tx::RENDERER_READY => {
            let now = msg.arg(0).ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let elapsed = app.started().elapsed().as_millis() as f64;
            let stats = json!({
                "mainReadyTime": 0.0,
                "loadTime": elapsed,
                "rendererReadyTime": now,
            });
            state.launch_stats = serde_json::from_value(stats.clone()).ok();
            app.emit(event::LAUNCH_TIMING_STATS, vec![stats]);
            return Ok(true);
        }
        tx::GET_APP_MENU => {
            app.emit(
                event::APP_MENU,
                vec![serde_json::to_value(&state.app_menu)?],
            );
        }
        tx::UPDATE_MENU_STATE => {
            let updates: Vec<MenuStateUpdate> =
                serde_json::from_value(msg.arg(0).cloned().unwrap_or(json!([])))?;
            // Only send the menu back when something actually changed. The
            // renderer re-sends its menu state on every store update, so an
            // unconditional reply is a never-ending loop between the two
            // (the Electron main process guarded against the same thing).
            if !apply_menu_state(&mut state.app_menu.items, &updates) {
                return Ok(false);
            }
            app.emit(
                event::APP_MENU,
                vec![serde_json::to_value(&state.app_menu)?],
            );
            return Ok(true);
        }
        tx::UPDATE_PREFERRED_APP_MENU_ITEM_LABELS => {
            // `MenuLabelsEvent`: renderer state (selected editor, force-push,
            // ...) that changes the wording of some items. Rebuild the menu
            // and keep which items the renderer had disabled.
            let labels: crate::menu::MenuLabels =
                serde_json::from_value(msg.arg(0).cloned().unwrap_or(json!({})))?;
            if labels == app.menu_labels() {
                return Ok(false);
            }
            let mut rebuilt = crate::menu::build(&labels);
            crate::menu::merge_state(&mut rebuilt, &state.app_menu.items);
            state.app_menu.items = rebuilt;
            app.set_menu_labels(labels);
            app.emit(
                event::APP_MENU,
                vec![serde_json::to_value(&state.app_menu)?],
            );
            return Ok(true);
        }
        tx::EXECUTE_MENU_ITEM_BY_ID => {
            app.emit(event::MENU_EVENT, vec![json!(msg.string(0)?)]);
        }
        tx::LOG => {
            let level = msg.optional_string(0).unwrap_or_else(|| "info".into());
            let message = msg.optional_string(1).unwrap_or_default();
            crate::log_renderer(&level, &message);
        }
        tx::WILL_QUIT => {
            state.quit_flags.quitting = true;
            return Ok(true);
        }
        tx::WILL_QUIT_EVEN_IF_UPDATING => {
            state.quit_flags.quitting = true;
            state.quit_flags.quitting_even_if_updating = true;
            return Ok(true);
        }
        tx::CANCEL_QUITTING => {
            state.quit_flags = Default::default();
            return Ok(true);
        }
        tx::UPDATE_ACCOUNTS => {
            if let Ok(tokens) = serde_json::from_value::<Vec<crate::models::account::EndpointToken>>(
                msg.arg(0).cloned().unwrap_or(json!([])),
            ) {
                state.endpoint_tokens = tokens;
            }
            return Ok(true);
        }
        tx::RESOLVED_IMAGE_TOKEN => {
            // Correlated reply to `resolve-image-token`; the authenticated
            // image filter is host-side in the Rust port, so just log it.
            tracing::debug!("resolved-image-token: {:?}", msg.args);
        }
        tx::SEND_ERROR_REPORT | tx::UNCAUGHT_EXCEPTION => {
            tracing::error!("renderer error report: {:?}", msg.args);
        }
        tx::DIALOG_DID_OPEN | tx::SHOW_CERTIFICATE_TRUST_DIALOG => {}
        tx::CRASH_READY => {
            app.emit(event::ERROR, vec![json!({
                "type": "generic",
                "error": { "message": "The renderer process crashed", "name": "Error", "stack": null }
            })]);
        }
        tx::CRASH_QUIT => {
            win(app, WindowAction::Quit)?;
        }
        // Push-only channels should never be sent by the renderer. Accept them
        // as no-ops so a stale renderer cannot break startup.
        other if event::ALL.contains(&other) => {}
        other if crate::ipc::channels::is_menu_event(other) => {
            app.emit(event::MENU_EVENT, vec![json!(other)]);
        }
        other => return Err(AppError::UnknownChannel(other.to_string())),
    }
    Ok(false)
}

fn apply_menu_state(items: &mut [SerializableMenuItem], updates: &[MenuStateUpdate]) -> bool {
    fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
        if *slot == value {
            return false;
        }
        *slot = value;
        true
    }

    let mut changed = false;
    for item in items {
        for update in updates {
            if item.id == update.id {
                if let Some(enabled) = update.state.enabled {
                    changed |= set(&mut item.enabled, enabled);
                }
                if let Some(checked) = update.state.checked {
                    changed |= set(&mut item.checked, checked);
                }
                if let Some(visible) = update.state.visible {
                    changed |= set(&mut item.visible, visible);
                }
                if let Some(label) = &update.state.label {
                    changed |= set(&mut item.label, label.clone());
                }
            }
        }
        changed |= apply_menu_state(&mut item.submenu, updates);
    }
    changed
}

pub fn notification_permission_default() -> DesktopNotificationPermission {
    DesktopNotificationPermission::Default
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, enabled: bool, submenu: Vec<SerializableMenuItem>) -> SerializableMenuItem {
        serde_json::from_value(json!({
            "id": id,
            "enabled": enabled,
            "visible": true,
            "menu": { "items": submenu },
        }))
        .unwrap()
    }

    fn update(id: &str, enabled: bool) -> MenuStateUpdate {
        serde_json::from_value(json!({ "id": id, "state": { "enabled": enabled } })).unwrap()
    }

    /// The renderer re-sends its menu state after every store update. Replying
    /// to an update that changes nothing would ping-pong forever.
    #[test]
    fn menu_state_reports_a_change_only_when_something_changed() {
        let mut menu = vec![item("repository", true, vec![item("push", false, vec![])])];

        assert!(apply_menu_state(&mut menu, &[update("push", true)]));
        assert!(menu[0].submenu[0].enabled);

        assert!(!apply_menu_state(&mut menu, &[update("push", true)]));
        assert!(!apply_menu_state(&mut menu, &[update("unknown", false)]));
        assert!(!apply_menu_state(&mut menu, &[]));
    }
}
