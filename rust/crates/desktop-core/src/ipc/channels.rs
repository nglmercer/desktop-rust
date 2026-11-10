//! The complete Electron IPC channel contract, transcribed from
//! `app/src/lib/ipc-shared.ts` and `app/src/main-process/main.ts`.
//!
//! 28 duplex (`invoke`/`handle`) channels, 51 simplex (`send`/`on`) channels.
//! 20 of the simplex channels are main-process -> renderer pushes and have no
//! inbound handler; they live in [`event`] and are emitted by Rust.

/// Duplex channels: renderer calls, Rust answers. (`RequestResponseChannels`)
pub mod invoke {
    pub const GET_PATH: &str = "get-path";
    pub const GET_APP_ARCHITECTURE: &str = "get-app-architecture";
    pub const GET_APP_PATH: &str = "get-app-path";
    pub const GET_EXEC_PATH: &str = "get-exec-path";
    pub const IS_RUNNING_UNDER_ARM64_TRANSLATION: &str = "is-running-under-arm64-translation";
    pub const MOVE_TO_TRASH: &str = "move-to-trash";
    pub const SHOW_ITEM_IN_FOLDER: &str = "show-item-in-folder";
    pub const WRITE_CLIPBOARD_TEXT: &str = "write-clipboard-text";
    pub const CONFIRM_REVEAL_DIRECTORY: &str = "confirm-reveal-directory";
    pub const SHOW_CONTEXTUAL_MENU: &str = "show-contextual-menu";
    pub const IS_WINDOW_FOCUSED: &str = "is-window-focused";
    pub const OPEN_EXTERNAL: &str = "open-external";
    pub const IS_IN_APPLICATION_FOLDER: &str = "is-in-application-folder";
    pub const MOVE_TO_APPLICATIONS_FOLDER: &str = "move-to-applications-folder";
    pub const CHECK_FOR_UPDATES: &str = "check-for-updates";
    pub const GET_CURRENT_WINDOW_STATE: &str = "get-current-window-state";
    pub const GET_CURRENT_WINDOW_ZOOM_FACTOR: &str = "get-current-window-zoom-factor";
    pub const RESOLVE_PROXY: &str = "resolve-proxy";
    pub const SHOW_SAVE_DIALOG: &str = "show-save-dialog";
    pub const SHOW_OPEN_DIALOG: &str = "show-open-dialog";
    pub const IS_WINDOW_MAXIMIZED: &str = "is-window-maximized";
    pub const GET_APPLE_ACTION_ON_DOUBLE_CLICK: &str = "get-apple-action-on-double-click";
    pub const SHOULD_USE_DARK_COLORS: &str = "should-use-dark-colors";
    pub const SAVE_GUID: &str = "save-guid";
    pub const GET_GUID: &str = "get-guid";
    pub const SHOW_NOTIFICATION: &str = "show-notification";
    pub const GET_NOTIFICATIONS_PERMISSION: &str = "get-notifications-permission";
    pub const REQUEST_NOTIFICATIONS_PERMISSION: &str = "request-notifications-permission";

    pub const ALL: &[&str] = &[
        GET_PATH,
        GET_APP_ARCHITECTURE,
        GET_APP_PATH,
        GET_EXEC_PATH,
        IS_RUNNING_UNDER_ARM64_TRANSLATION,
        MOVE_TO_TRASH,
        SHOW_ITEM_IN_FOLDER,
        WRITE_CLIPBOARD_TEXT,
        CONFIRM_REVEAL_DIRECTORY,
        SHOW_CONTEXTUAL_MENU,
        IS_WINDOW_FOCUSED,
        OPEN_EXTERNAL,
        IS_IN_APPLICATION_FOLDER,
        MOVE_TO_APPLICATIONS_FOLDER,
        CHECK_FOR_UPDATES,
        GET_CURRENT_WINDOW_STATE,
        GET_CURRENT_WINDOW_ZOOM_FACTOR,
        RESOLVE_PROXY,
        SHOW_SAVE_DIALOG,
        SHOW_OPEN_DIALOG,
        IS_WINDOW_MAXIMIZED,
        GET_APPLE_ACTION_ON_DOUBLE_CLICK,
        SHOULD_USE_DARK_COLORS,
        SAVE_GUID,
        GET_GUID,
        SHOW_NOTIFICATION,
        GET_NOTIFICATIONS_PERMISSION,
        REQUEST_NOTIFICATIONS_PERMISSION,
    ];
}

/// Simplex channels: renderer fires, Rust consumes. (`RequestChannels` minus
/// the push-only channels listed in [`event`])
pub mod send {
    pub const SELECT_ALL_WINDOW_CONTENTS: &str = "select-all-window-contents";
    pub const DIALOG_DID_OPEN: &str = "dialog-did-open";
    pub const UPDATE_MENU_STATE: &str = "update-menu-state";
    pub const RENDERER_READY: &str = "renderer-ready";
    pub const EXECUTE_MENU_ITEM_BY_ID: &str = "execute-menu-item-by-id";
    pub const SHOW_CERTIFICATE_TRUST_DIALOG: &str = "show-certificate-trust-dialog";
    pub const GET_APP_MENU: &str = "get-app-menu";
    pub const UPDATE_PREFERRED_APP_MENU_ITEM_LABELS: &str = "update-preferred-app-menu-item-labels";
    pub const UNCAUGHT_EXCEPTION: &str = "uncaught-exception";
    pub const SEND_ERROR_REPORT: &str = "send-error-report";
    pub const UNSAFE_OPEN_DIRECTORY: &str = "unsafe-open-directory";
    pub const LOG: &str = "log";
    pub const WILL_QUIT: &str = "will-quit";
    pub const WILL_QUIT_EVEN_IF_UPDATING: &str = "will-quit-even-if-updating";
    pub const CANCEL_QUITTING: &str = "cancel-quitting";
    pub const CRASH_READY: &str = "crash-ready";
    pub const CRASH_QUIT: &str = "crash-quit";
    pub const UPDATE_ACCOUNTS: &str = "update-accounts";
    pub const RESOLVED_IMAGE_TOKEN: &str = "resolved-image-token";
    pub const QUIT_AND_INSTALL_UPDATES: &str = "quit-and-install-updates";
    pub const QUIT_APP: &str = "quit-app";
    pub const MINIMIZE_WINDOW: &str = "minimize-window";
    pub const MAXIMIZE_WINDOW: &str = "maximize-window";
    pub const UNMAXIMIZE_WINDOW: &str = "unmaximize-window";
    pub const CLOSE_WINDOW: &str = "close-window";
    pub const SET_WINDOW_ZOOM_FACTOR: &str = "set-window-zoom-factor";
    pub const INSTALL_WINDOWS_CLI: &str = "install-windows-cli";
    pub const UNINSTALL_WINDOWS_CLI: &str = "uninstall-windows-cli";
    pub const SHOW_CERTIFICATE_TRUST_DIALOG_ALIAS: &str = "show-certificate-trust-dialog";
    pub const LOG_ALIAS: &str = "log";
    pub const FOCUS_WINDOW: &str = "focus-window";
    pub const SET_NATIVE_THEME_SOURCE: &str = "set-native-theme-source";
    pub const UPDATE_WINDOW_BACKGROUND_COLOR: &str = "update-window-background-color";

    pub const ALL: &[&str] = &[
        SELECT_ALL_WINDOW_CONTENTS,
        DIALOG_DID_OPEN,
        UPDATE_MENU_STATE,
        RENDERER_READY,
        EXECUTE_MENU_ITEM_BY_ID,
        SHOW_CERTIFICATE_TRUST_DIALOG,
        GET_APP_MENU,
        UPDATE_PREFERRED_APP_MENU_ITEM_LABELS,
        UNCAUGHT_EXCEPTION,
        SEND_ERROR_REPORT,
        UNSAFE_OPEN_DIRECTORY,
        LOG,
        WILL_QUIT,
        WILL_QUIT_EVEN_IF_UPDATING,
        CANCEL_QUITTING,
        CRASH_READY,
        CRASH_QUIT,
        UPDATE_ACCOUNTS,
        RESOLVED_IMAGE_TOKEN,
        QUIT_AND_INSTALL_UPDATES,
        QUIT_APP,
        MINIMIZE_WINDOW,
        MAXIMIZE_WINDOW,
        UNMAXIMIZE_WINDOW,
        CLOSE_WINDOW,
        SET_WINDOW_ZOOM_FACTOR,
        INSTALL_WINDOWS_CLI,
        UNINSTALL_WINDOWS_CLI,
        FOCUS_WINDOW,
        SET_NATIVE_THEME_SOURCE,
        UPDATE_WINDOW_BACKGROUND_COLOR,
    ];
}

/// Push-only channels: Rust -> webview. These have no `ipcMain.on` handler in
/// Electron; they are emitted with `ipcWebContents.send`.
pub mod event {
    pub const MENU_EVENT: &str = "menu-event";
    pub const URL_ACTION: &str = "url-action";
    pub const CLI_ACTION: &str = "cli-action";
    pub const LAUNCH_TIMING_STATS: &str = "launch-timing-stats";
    pub const APP_MENU: &str = "app-menu";
    pub const CERTIFICATE_ERROR: &str = "certificate-error";
    pub const FOCUS: &str = "focus";
    pub const BLUR: &str = "blur";
    pub const NATIVE_THEME_UPDATED: &str = "native-theme-updated";
    pub const WINDOW_STATE_CHANGED: &str = "window-state-changed";
    pub const ZOOM_FACTOR_CHANGED: &str = "zoom-factor-changed";
    pub const ERROR: &str = "error";
    pub const AUTO_UPDATER_ERROR: &str = "auto-updater-error";
    pub const AUTO_UPDATER_CHECKING_FOR_UPDATE: &str = "auto-updater-checking-for-update";
    pub const AUTO_UPDATER_UPDATE_AVAILABLE: &str = "auto-updater-update-available";
    pub const AUTO_UPDATER_UPDATE_NOT_AVAILABLE: &str = "auto-updater-update-not-available";
    pub const AUTO_UPDATER_UPDATE_DOWNLOADED: &str = "auto-updater-update-downloaded";
    pub const SHOW_INSTALLING_UPDATE: &str = "show-installing-update";
    pub const NOTIFICATION_EVENT: &str = "notification-event";
    pub const RESOLVE_IMAGE_TOKEN: &str = "resolve-image-token";

    pub const ALL: &[&str] = &[
        MENU_EVENT,
        URL_ACTION,
        CLI_ACTION,
        LAUNCH_TIMING_STATS,
        APP_MENU,
        CERTIFICATE_ERROR,
        FOCUS,
        BLUR,
        NATIVE_THEME_UPDATED,
        WINDOW_STATE_CHANGED,
        ZOOM_FACTOR_CHANGED,
        ERROR,
        AUTO_UPDATER_ERROR,
        AUTO_UPDATER_CHECKING_FOR_UPDATE,
        AUTO_UPDATER_UPDATE_AVAILABLE,
        AUTO_UPDATER_UPDATE_NOT_AVAILABLE,
        AUTO_UPDATER_UPDATE_DOWNLOADED,
        SHOW_INSTALLING_UPDATE,
        NOTIFICATION_EVENT,
        RESOLVE_IMAGE_TOKEN,
    ];
}

/// The three channels that Electron implements with `sendSync` + `returnValue`.
/// wry has no synchronous IPC, so the JS bridge awaits them explicitly.
pub const SYNCHRONOUS_CHANNELS: &[&str] = &[
    send::WILL_QUIT,
    send::WILL_QUIT_EVEN_IF_UPDATING,
    send::CANCEL_QUITTING,
];

/// `MenuEvent` from `app/src/main-process/menu/menu-event.ts`. Production events
/// only; the `test-*` union members are intentionally omitted from the compiled
/// list but accepted at runtime by [`is_menu_event`].
pub const MENU_EVENTS: &[&str] = &[
    "push",
    "force-push",
    "pull",
    "fetch",
    "show-changes",
    "show-history",
    "add-local-repository",
    "create-branch",
    "show-branches",
    "show-worktrees",
    "create-worktree",
    "remove-repository",
    "create-repository",
    "rename-branch",
    "delete-branch",
    "discard-all-changes",
    "stash-all-changes",
    "show-preferences",
    "choose-repository",
    "open-working-directory",
    "update-branch-with-contribution-target-branch",
    "compare-to-branch",
    "merge-branch",
    "squash-and-merge-branch",
    "rebase-branch",
    "show-repository-settings",
    "open-in-shell",
    "compare-on-github",
    "branch-on-github",
    "view-repository-on-github",
    "clone-repository",
    "show-about",
    "go-to-commit-message",
    "open-pull-request",
    "install-darwin-cli",
    "install-windows-cli",
    "uninstall-windows-cli",
    "open-external-editor",
    "open-in-copilot-app",
    "open-with-external-editor",
    "select-all",
    "show-stashed-changes",
    "hide-stashed-changes",
    "find-text",
    "create-issue-in-repository-on-github",
    "preview-pull-request",
    "test-app-error",
    "decrease-active-resizable-width",
    "increase-active-resizable-width",
    "toggle-changes-filter",
];

pub fn is_menu_event(value: &str) -> bool {
    MENU_EVENTS.contains(&value) || value.starts_with("test-") || value == "boomtown"
}

pub fn is_known(channel: &str) -> bool {
    invoke::ALL.contains(&channel)
        || send::ALL.contains(&channel)
        || event::ALL.contains(&channel)
        || is_menu_event(channel)
}
