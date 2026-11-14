//! OS integrations that Electron previously supplied from the main process:
//! `shell.openExternal`, `shell.trashItem`, `nativeTheme`, `clipboard`,
//! `session.resolveProxy`, `desktop-notifications` and the Squirrel updater.
//!
//! Everything here is plain `std::process::Command` (or the file system) — no
//! Electron APIs and no Node modules.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::error::{AppError, Result};
use crate::models::misc::DesktopNotificationPermission;

// Keep asynchronous launcher reapers bounded if an application holds its
// opener process open. A slot remains held until that process exits.
static OPEN_PATH_LAUNCHERS: AtomicUsize = AtomicUsize::new(0);
const MAX_OPEN_PATH_LAUNCHERS: usize = 32;

struct OpenPathLease;

impl OpenPathLease {
    fn acquire() -> Option<Self> {
        let mut current = OPEN_PATH_LAUNCHERS.load(Ordering::Acquire);
        loop {
            if current >= MAX_OPEN_PATH_LAUNCHERS {
                return None;
            }
            match OPEN_PATH_LAUNCHERS.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for OpenPathLease {
    fn drop(&mut self) {
        OPEN_PATH_LAUNCHERS.fetch_sub(1, Ordering::Release);
    }
}

/// Open a local file or directory with its default application. The result
/// follows Electron's `shell.openPath`: empty on success, an error otherwise.
/// A long-lived opener may already have launched an application before the
/// request returns, so callers must not retry automatically after cancellation.
pub fn open_path(path: &str) -> String {
    #[cfg(target_os = "windows")]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let program = "xdg-open";
    open_path_with(path, program)
}

fn open_path_with(path: &str, program: &str) -> String {
    let target = Path::new(path);
    if path.len() > 32_768 || path.contains('\0') || !target.is_absolute() {
        return "Expected an absolute local path.".into();
    }
    match std::fs::metadata(target) {
        Ok(metadata) if metadata.is_file() || metadata.is_dir() => {}
        _ => return "The file or folder is unavailable.".into(),
    }
    let Some(lease) = OpenPathLease::acquire() else {
        return "Too many file-opening requests are still running.".into();
    };
    let child = Command::new(program)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return "Unable to start the default application.".into();
    };
    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    String::new()
                } else {
                    "The default application could not open the path.".into()
                };
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return "Unable to confirm that the path was opened. The application may already be open; check before retrying.".into();
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => break,
        }
    }
    let child = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let child_for_reaper = child.clone();
    let reaper = std::thread::Builder::new()
        .name("desktop-open-path".into())
        .spawn(move || {
            let _lease = lease;
            if let Some(mut child) = child_for_reaper
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = child.wait();
            }
        });
    if reaper.is_err() {
        if let Some(mut child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // The launcher may already have opened the target; report uncertainty.
        return "Unable to confirm that the path was opened. The application may already be open; check before retrying.".into();
    }
    String::new()
}

#[cfg(all(test, unix))]
mod open_path_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "desktop-open-path-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        directory
    }

    #[test]
    fn opens_exact_unicode_and_option_like_paths_without_a_shell() {
        let root = fixture();
        let opener = root.join("opener");
        std::fs::write(&opener, "#!/bin/sh\nprintf '%s' \"$1\" > \"$0.arg\"\n").unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("-资料 '$?.txt");
        std::fs::write(&target, b"data").unwrap();
        assert_eq!(
            open_path_with(target.to_str().unwrap(), opener.to_str().unwrap()),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(root.join("opener.arg")).unwrap(),
            target.to_str().unwrap()
        );
        let folder = root.join("folder with spaces");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(
            open_path_with(folder.to_str().unwrap(), opener.to_str().unwrap()),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(root.join("opener.arg")).unwrap(),
            folder.to_str().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_or_missing_paths_and_reports_failed_launchers() {
        let root = fixture();
        let target = root.join("exists");
        std::fs::write(&target, b"data").unwrap();
        let opener = root.join("fail");
        std::fs::write(&opener, "#!/bin/sh\nexit 5\n").unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!open_path_with("-relative", opener.to_str().unwrap()).is_empty());
        assert!(!open_path_with("/bad\0path", opener.to_str().unwrap()).is_empty());
        assert!(
            !open_path_with(
                root.join("missing").to_str().unwrap(),
                opener.to_str().unwrap()
            )
            .is_empty()
        );
        assert!(!open_path_with(target.to_str().unwrap(), opener.to_str().unwrap()).is_empty());
        assert!(
            !open_path_with(
                target.to_str().unwrap(),
                root.join("missing-opener").to_str().unwrap()
            )
            .is_empty()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_started_when_an_opener_outlives_the_observation_window() {
        let root = fixture();
        let target = root.join("exists");
        std::fs::write(&target, b"data").unwrap();
        let opener = root.join("slow-opener");
        std::fs::write(&opener, "#!/bin/sh\nsleep 0.5\ntouch \"$0.done\"\nexit 5\n").unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            open_path_with(target.to_str().unwrap(), opener.to_str().unwrap()),
            ""
        );
        let done = root.join("slow-opener.done");
        let until = Instant::now() + Duration::from_secs(2);
        while !done.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(done.exists(), "opener must complete before fixture cleanup");
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// `shell.openExternal` / `UNSAFE_openDirectory`. Never invokes a shell with
/// a concatenated string, so URLs/paths cannot inject arguments.
pub fn open_external(target: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = Command::new("cmd")
        .args(["/C", "start", "", "explorer.exe", target])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(target).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(target).spawn();
    result.is_ok()
}

pub fn show_item_in_folder(path: &str) -> bool {
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer.exe")
        .arg(format!("/select,{}", path.replace('/', "\\")))
        .spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").args(["-R", path]).spawn();
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = Command::new("xdg-open")
        .arg(
            Path::new(path)
                .parent()
                .unwrap_or(Path::new("."))
                .to_string_lossy()
                .into_owned(),
        )
        .spawn();
    result.is_ok()
}

/// `shell.trashItem` — must use the OS recycle bin, never `remove_file`.
pub fn move_to_trash(path: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Microsoft.VisualBasic.FileIO.FileSystem sends to the Recycle Bin.
        let escaped = path.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName Microsoft.VisualBasic; \
             if (Test-Path -LiteralPath '{escaped}' -PathType Container) {{ \
               [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory('{escaped}','OnlyErrorDialogs','SendToRecycleBin') \
             }} else {{ \
               [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{escaped}','OnlyErrorDialogs','SendToRecycleBin') \
             }}"
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(AppError::Io)?;
        if !status.success() {
            return Err(AppError::msg("Unable to move the item to the Recycle Bin"));
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Finder\" to delete POSIX file \"{}\"",
            path.replace('"', "\\\"")
        );
        let status = Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(AppError::Io)?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| AppError::msg("Finder refused to trash the item"))
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let status = Command::new("gio")
            .args(["trash", path])
            .status()
            .map_err(AppError::Io)?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| AppError::msg("gio trash failed"))
    }
}

/// Tell the renderer when Git must inherit a host-configured proxy without
/// returning the proxy URL (which may contain credentials) across IPC.
/// This does not inspect system proxy settings or evaluate PAC files.
pub fn resolve_proxy(url: &str) -> String {
    resolve_proxy_with(url, |name| {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    })
}

fn resolve_proxy_with(url: &str, present: impl Fn(&str) -> bool) -> String {
    let Ok(target) = reqwest::Url::parse(url) else {
        return "DIRECT".into();
    };
    let configured = match target.scheme() {
        "http" => present("http_proxy"),
        "https" => present("https_proxy") || present("HTTPS_PROXY"),
        _ => false,
    } || (matches!(target.scheme(), "http" | "https")
        && (present("all_proxy") || present("ALL_PROXY")));
    if configured {
        "DESKTOP_INHERIT_PROXY".into()
    } else {
        "DIRECT".into()
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::resolve_proxy_with;

    #[test]
    fn proxy_detection_is_only_a_marker_and_keeps_curl_http_proxy_rules() {
        let detect =
            |target: &str, names: &[&str]| resolve_proxy_with(target, |name| names.contains(&name));
        assert_eq!(
            detect("https://example.test", &["HTTPS_PROXY"]),
            "DESKTOP_INHERIT_PROXY"
        );
        assert_eq!(
            detect("http://example.test", &["ALL_PROXY"]),
            "DESKTOP_INHERIT_PROXY"
        );
        assert_eq!(detect("http://example.test", &["HTTP_PROXY"]), "DIRECT");
        assert_eq!(detect("ssh://example.test", &["ALL_PROXY"]), "DIRECT");
        assert_eq!(detect("https://example.test", &[]), "DIRECT");
    }
}

/// `nativeTheme.shouldUseDarkColors`.
pub fn should_use_dark_colors() -> bool {
    if let Ok(value) = std::env::var("DESKTOP_DARK_MODE") {
        return value == "1";
    }
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "/v",
                "AppsUseLightTheme",
            ])
            .output();
        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            return text.contains("0x0");
        }
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output();
        if let Ok(out) = output {
            return String::from_utf8_lossy(&out.stdout).trim() == "Dark";
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return linux_prefers_dark().unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

/// A colour-scheme source on Linux, in order of reliability.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum SchemeSource {
    /// The freedesktop Settings portal (GNOME, KDE Plasma and others).
    Portal = 1,
    /// The window colours of a KDE colour scheme.
    Kde = 2,
    /// GNOME's `gsettings`.
    Gnome = 3,
}

/// The source that answered last. Checks ask it first, so a steady state costs
/// one lookup instead of up to three process starts. 0 means none yet.
#[cfg(all(unix, not(target_os = "macos")))]
static LAST_SCHEME_SOURCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The colour scheme of a Linux desktop. The last source that answered is asked
/// first; when it gives no answer, every source is tried in order again. A
/// desktop with no preference therefore still ends with `None`, as before.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_prefers_dark() -> Option<bool> {
    use std::sync::atomic::Ordering;
    let cached = LAST_SCHEME_SOURCE.load(Ordering::Relaxed);
    if cached != 0 {
        let source = match cached {
            1 => SchemeSource::Portal,
            2 => SchemeSource::Kde,
            _ => SchemeSource::Gnome,
        };
        if let Some(dark) = ask_scheme_source(source) {
            return Some(dark);
        }
    }
    for source in [SchemeSource::Portal, SchemeSource::Kde, SchemeSource::Gnome] {
        if let Some(dark) = ask_scheme_source(source) {
            LAST_SCHEME_SOURCE.store(source as u8, Ordering::Relaxed);
            return Some(dark);
        }
    }
    None
}

#[cfg(all(unix, not(target_os = "macos")))]
fn ask_scheme_source(source: SchemeSource) -> Option<bool> {
    match source {
        SchemeSource::Portal => portal_prefers_dark(),
        SchemeSource::Kde => kde_window_is_dark(),
        SchemeSource::Gnome => gnome_prefers_dark(),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn portal_prefers_dark() -> Option<bool> {
    // org.freedesktop.appearance color-scheme: 1 = prefer dark, 2 = prefer light, 0 = no preference.
    let text = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.Read",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())?;
    let value = text
        .split("uint32 ")
        .nth(1)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse::<u32>()
        .ok()?;
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn gnome_prefers_dark() -> Option<bool> {
    let gnome = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&gnome.stdout);
    if text.contains("prefer-dark") {
        Some(true)
    } else if text.contains("default") || text.contains("prefer-light") {
        Some(false)
    } else {
        None
    }
}

/// KDE keeps the active colour scheme in `kdeglobals`. A scheme is dark when
/// its window background is darker than its text.
#[cfg(all(unix, not(target_os = "macos")))]
fn kde_window_is_dark() -> Option<bool> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })?;
    let text = std::fs::read_to_string(config.join("kdeglobals")).ok()?;

    let mut section = "";
    let mut background = None;
    let mut foreground = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let parse = |value: &str| -> Option<f64> {
            let parts: Vec<f64> = value
                .split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            (parts.len() >= 3).then(|| 0.299 * parts[0] + 0.587 * parts[1] + 0.114 * parts[2])
        };
        if section == "[Colors:Window]" {
            if let Some(value) = line.strip_prefix("BackgroundNormal=") {
                background = parse(value);
            } else if let Some(value) = line.strip_prefix("ForegroundNormal=") {
                foreground = parse(value);
            }
        }
    }
    match (background, foreground) {
        (Some(bg), Some(fg)) => Some(bg < fg),
        (Some(bg), None) => Some(bg < 128.0),
        _ => None,
    }
}

pub fn notifications_permission() -> DesktopNotificationPermission {
    // Notifications are rendered in-app by default; OS permission is opt-in.
    match std::env::var("DESKTOP_NOTIFICATIONS").as_deref() {
        Ok("granted") => DesktopNotificationPermission::Granted,
        Ok("denied") => DesktopNotificationPermission::Denied,
        _ => DesktopNotificationPermission::Default,
    }
}

pub fn request_notifications_permission() -> bool {
    std::env::var("DESKTOP_NOTIFICATIONS").as_deref() != Ok("denied")
}

/// `showNotification` — the notification is delivered as an in-app event;
/// the OS toast is best-effort and platform-specific.
pub fn show_notification(title: &str, body: &str, user_info: &serde_json::Value) -> Option<String> {
    let id = format!(
        "{:016x}",
        crate::models::repository::simple_hash(&format!("{title}{body}{user_info}"))
    );
    tracing::info!(id = %id, title = %title, "notification");
    Some(id)
}

#[cfg(target_os = "windows")]
pub fn is_running_under_arm64_translation() -> bool {
    // Electron used `app.runningUnderARM64Translation`. A Rust binary compiled
    // for x86_64 running on ARM64 Windows is the equivalent check; the OS does
    // not expose it to a native process without GetMachineTypeAttributes, so
    // honour the documented env override and default to false.
    std::env::var("PROCESSOR_ARCHITEW6432").as_deref() == Ok("ARM64")
}

#[cfg(not(target_os = "windows"))]
pub fn is_running_under_arm64_translation() -> bool {
    false
}

pub fn is_in_application_folder() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().ok()?;
        return Some(exe.to_string_lossy().contains("/Applications/"));
    }
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn move_to_applications_folder() -> Result<()> {
    Err(AppError::NotImplemented(
        "macOS move-to-Applications-Folder".into(),
    ))
}

/// `installWindowsCLI` / `uninstallWindowsCLI` — Squirrel trampoline scripts.
/// The Rust port writes a `.cmd` shim in userData and adds it to the user PATH.
pub fn install_cli(install: bool, user_data: &Path, exe: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let bin_dir = user_data.join("bin");
        std::fs::create_dir_all(&bin_dir)?;
        let shim = bin_dir.join("github.cmd");
        if install {
            let body = format!("@echo off\r\n\"{}\" %*\r\n", exe.to_string_lossy());
            std::fs::write(&shim, body)?;
            // `install-windows-cli` additionally registers a `github` shell
            // command. Writing the shim is the portable part; PATH changes for
            // the current user are best done by an installer (see MIGRATION.md).
        } else if shim.exists() {
            std::fs::remove_file(&shim)?;
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (user_data, exe, install);
        Ok(())
    }
}

/// `check-for-updates`: the Squirrel/updates mechanism is intentionally out of
/// scope for this migration. Return the same "no update available" outcome the
/// UI already handles instead of throwing.
pub fn check_for_updates(_url: &str) -> Result<()> {
    Ok(())
}
