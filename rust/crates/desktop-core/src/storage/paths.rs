//! Port of `app.getPath` (`main.ts` `get-path` handler) and the log/storage
//! path helpers in `app/src/lib/logging/`.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::models::misc::PathType;

/// Deliberately not Electron's "GitHub Desktop": an installed GitHub Desktop
/// keeps its own state there, and the two must not write over each other.
const APP_DATA_DIR: &str = "GitHub Desktop Rust";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub user_data: PathBuf,
    pub app_path: PathBuf,
    pub exec_path: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Result<Self> {
        let user_data = if let Ok(override_dir) = std::env::var("DESKTOP_USER_DATA") {
            PathBuf::from(override_dir)
        } else {
            #[cfg(target_os = "windows")]
            {
                std::env::var("APPDATA")
                    .map(|p| PathBuf::from(p).join(APP_DATA_DIR))
                    .unwrap_or_else(|_| PathBuf::from(".").join(".desktop-data"))
            }
            #[cfg(target_os = "macos")]
            {
                home().join("Library").join("Application Support").join(APP_DATA_DIR)
            }
            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            {
                std::env::var("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home().join(".config"))
                    .join(APP_DATA_DIR)
            }
        };

        std::fs::create_dir_all(&user_data)?;
        std::fs::create_dir_all(user_data.join("logs"))?;
        std::fs::create_dir_all(user_data.join("credentials"))?;

        let exec_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("github-desktop-rs"));
        let app_path = exec_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        Ok(Self { user_data, app_path, exec_path })
    }

    pub fn state_file(&self) -> PathBuf { self.user_data.join("state.json") }
    pub fn preferences_file(&self) -> PathBuf { self.user_data.join("preferences.json") }
    pub fn key_value_file(&self) -> PathBuf { self.user_data.join("local-storage.json") }
    pub fn guid_file(&self) -> PathBuf { self.user_data.join("guid") }
    pub fn log_dir(&self) -> PathBuf { self.user_data.join("logs") }
    pub fn credential_dir(&self) -> PathBuf { self.user_data.join("credentials") }
    pub fn crash_dumps(&self) -> PathBuf { self.user_data.join("Crashpad").join("reports") }

    pub fn get_path(&self, kind: PathType) -> String {
        let home = home();
        let path = match kind {
            PathType::Home | PathType::Documents => home.clone(),
            PathType::AppData | PathType::UserData => self.user_data.clone(),
            PathType::Cache => self.user_data.join("Cache"),
            PathType::Temp => std::env::temp_dir(),
            PathType::Downloads => home.join("Downloads"),
            PathType::Desktop => home.join("Desktop"),
            PathType::Exe | PathType::Module => self.exec_path.clone(),
            PathType::Logs => self.log_dir(),
            PathType::CrashDumps => self.crash_dumps(),
        };
        path.to_string_lossy().into_owned()
    }

    pub fn get_path_by_name(&self, name: &str) -> String {
        let kind = match name {
            "home" => PathType::Home,
            "appData" => PathType::AppData,
            "userData" => PathType::UserData,
            "cache" => PathType::Cache,
            "temp" => PathType::Temp,
            "documents" => PathType::Documents,
            "downloads" => PathType::Downloads,
            "desktop" => PathType::Desktop,
            "exe" => PathType::Exe,
            "module" => PathType::Module,
            "logs" => PathType::Logs,
            "crashDumps" => PathType::CrashDumps,
            _ => return self.user_data.join(name).to_string_lossy().into_owned(),
        };
        self.get_path(kind)
    }
}

#[cfg(target_os = "windows")]
fn home() -> PathBuf {
    std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(not(target_os = "windows"))]
fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}
