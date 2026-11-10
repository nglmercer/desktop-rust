//! Wire protocol for the wry bridge.
//!
//! Electron used `ipcRenderer.invoke` for duplex calls and `ipcRenderer.send`
//! for fire-and-forget. wry exposes two primitives that map cleanly onto them:
//!
//! | Electron            | wry transport                                   |
//! |---------------------|-------------------------------------------------|
//! | `ipcRenderer.invoke`| `fetch("desktop://localhost/__invoke", POST)`   |
//! | `ipcRenderer.send`  | `window.ipc.postMessage(json)`                   |
//! | `webContents.send`  | `WebView::evaluate_script("__desktop.emit(…)")` |
//!
//! The custom scheme matters: Electron loaded `file://index.html` with
//! `nodeIntegration: true`. wry has no Node, so we serve the web layer from a
//! `desktop://` custom protocol and inject the bridge with an initialization
//! script, which also gives us a safe same-origin boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, Result, RpcError};
use crate::models::{ThemeSource, WindowState};

/// Payload of both `send` and `invoke`. `args` is an array because Electron's
/// `ipcRenderer.send(channel, ...args)` is variadic and the typed wrappers in
/// `ipc-renderer.ts` simply forward it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcMessage {
    pub channel: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

impl IpcMessage {
    pub fn parse(body: &[u8]) -> Result<Self> {
        serde_json::from_slice(body).map_err(AppError::Json)
    }

    pub fn arg(&self, index: usize) -> Result<&Value> {
        self.args.get(index).ok_or_else(|| {
            AppError::BadArgs(format!("{} requires argument #{index}", self.channel))
        })
    }

    pub fn string(&self, index: usize) -> Result<String> {
        self.arg(index)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                AppError::BadArgs(format!(
                    "{}: argument #{index} must be a string",
                    self.channel
                ))
            })
    }

    pub fn optional_string(&self, index: usize) -> Option<String> {
        self.args
            .get(index)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResult {
    pub fn ok(value: Value) -> Self {
        Self {
            ok: true,
            value: Some(value),
            error: None,
        }
    }
    pub fn err(error: AppError) -> Self {
        Self {
            ok: false,
            value: None,
            error: Some(error.to_rpc_error()),
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_else(|_| b"{\"ok\":false}".to_vec())
    }
}

// ---------------------------------------------------------------------------
// Dialog / menu payloads (mirrors app/src/ui/main-process-proxy.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDialogOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_label: Option<String>,
    #[serde(default)]
    pub filters: Vec<FileFilter>,
    /// Electron "properties": openDirectory, openFile, multiSelections,
    /// createDirectory, showHiddenFiles, …
    #[serde(default)]
    pub properties: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDialogOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_field_label: Option<String>,
    #[serde(default)]
    pub filters: Vec<FileFilter>,
}

/// One entry of the application menu, shaped like the renderer's `IMenuItem`
/// (`app/src/models/app-menu.ts`): `type` selects the variant, and a submenu
/// carries its children as `menu.items`, as Electron's serialized menu does.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SerializableMenuItem {
    pub id: String,
    #[serde(default)]
    pub label: String,
    // Electron sends `null`, not an absent key, when there is no shortcut.
    #[serde(default)]
    pub accelerator: Option<String>,
    #[serde(default)]
    pub access_key: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub checked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// `menuItem`, `separator`, `submenuItem` or `checkbox`.
    #[serde(rename = "type", default = "normal_kind")]
    pub kind: String,
    #[serde(rename = "menu", default, with = "menu_items")]
    pub submenu: Vec<SerializableMenuItem>,
}

fn yes() -> bool {
    true
}

fn normal_kind() -> String {
    "menuItem".into()
}

/// A submenu's children travel as `{ "items": [...] }`, the renderer's `IMenu`.
mod menu_items {
    use super::SerializableMenuItem;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Menu {
        items: Vec<SerializableMenuItem>,
    }

    pub fn serialize<S: Serializer>(
        items: &[SerializableMenuItem],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        Menu {
            items: items.to_vec(),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<SerializableMenuItem>, D::Error> {
        Ok(Menu::deserialize(d)?.items)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuStateUpdate {
    pub id: String,
    #[serde(default)]
    pub state: MenuItemState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItemState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageOptions {
    pub message: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub buttons: Vec<String>,
    #[serde(default = "default_ok")]
    pub default_id: usize,
    #[serde(default = "default_cancel")]
    pub cancel_id: usize,
}

fn default_ok() -> usize {
    0
}
fn default_cancel() -> usize {
    1
}

// ---------------------------------------------------------------------------
// Window actions
// ---------------------------------------------------------------------------

/// Operations that require the OS window / UI thread. `desktop-core` never
/// depends on tao or wry; it sends these to the host over the event loop.
#[derive(Debug, Clone)]
pub enum WindowAction {
    Minimize,
    Maximize,
    Unmaximize,
    Close,
    Focus,
    Quit,
    QuitAndInstallUpdate,
    DevTools,
    Reload,
    SelectAll,
    SetZoomFactor(f64),
    SetBackgroundColor(String),
    SetTheme(ThemeSource),
    SetTitle(String),
    OpenExternal(String),
    ShowItemInFolder(String),
    UnsafeOpenDirectory(String),
    MoveToTrash(String),
    WriteClipboardText(String),
    ShowOpenDialog(OpenDialogOptions),
    ShowSaveDialog(SaveDialogOptions),
    ShowMessageBox(MessageOptions),
    ShowContextMenu(Vec<SerializableMenuItem>),
    InstallCli(bool),
    /// Queries, answered synchronously by the host.
    Query(WindowQuery),
}

#[derive(Debug, Clone, Copy)]
pub enum WindowQuery {
    IsMaximized,
    IsFocused,
    WindowState,
    ZoomFactor,
    ShouldUseDarkColors,
    IsInApplicationFolder,
}

impl WindowQuery {
    pub fn default_value(self) -> Value {
        match self {
            Self::IsMaximized | Self::IsFocused => Value::Bool(false),
            Self::WindowState => serde_json::to_value(WindowState::Normal).unwrap_or(Value::Null),
            Self::ZoomFactor => Value::from(1.0),
            Self::ShouldUseDarkColors => Value::Bool(false),
            Self::IsInApplicationFolder => Value::Null,
        }
    }
}

/// The seam between the pure-Rust core and the tao/wry shell.
pub trait Host: Send + Sync {
    /// Push a main-process event into the webview (`ipcWebContents.send`).
    fn emit(&self, channel: &str, args: Value);

    /// Run an OS-level action on the UI thread and return its result.
    fn window(&self, action: WindowAction) -> Result<Value>;
}

/// Used by unit tests and by the `--no-window` CLI path.
#[derive(Debug, Default)]
pub struct NullHost;

impl Host for NullHost {
    fn emit(&self, _channel: &str, _args: Value) {}
    fn window(&self, action: WindowAction) -> Result<Value> {
        Ok(match action {
            WindowAction::Query(q) => q.default_value(),
            WindowAction::ShowOpenDialog(_) | WindowAction::ShowSaveDialog(_) => Value::Null,
            WindowAction::ShowMessageBox(_) => Value::from(0),
            WindowAction::ShowContextMenu(_) => Value::Null,
            _ => Value::Null,
        })
    }
}
