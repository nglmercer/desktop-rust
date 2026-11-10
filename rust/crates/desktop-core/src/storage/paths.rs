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
    /// The Copilot SDK platform package (`copilot-sdk-<os>-<arch>`), which holds
    /// the standalone runtime under `prebuilds/`. `None` when it is not installed.
    pub copilot_dir: Option<PathBuf>,
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
                home()
                    .join("Library")
                    .join("Application Support")
                    .join(APP_DATA_DIR)
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

        let exec_path =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("github-desktop-rs"));
        let app_path = exec_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let copilot_dir = find_copilot_dir(&exec_path);

        Ok(Self {
            user_data,
            app_path,
            exec_path,
            copilot_dir,
        })
    }

    pub fn state_file(&self) -> PathBuf {
        self.user_data.join("state.json")
    }
    pub fn preferences_file(&self) -> PathBuf {
        self.user_data.join("preferences.json")
    }
    pub fn key_value_file(&self) -> PathBuf {
        self.user_data.join("local-storage.json")
    }
    pub fn guid_file(&self) -> PathBuf {
        self.user_data.join("guid")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.user_data.join("logs")
    }
    pub fn credential_dir(&self) -> PathBuf {
        self.user_data.join("credentials")
    }
    pub fn crash_dumps(&self) -> PathBuf {
        self.user_data.join("Crashpad").join("reports")
    }

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
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(not(target_os = "windows"))]
fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Node's name for this OS and CPU, as used by the Copilot SDK packages.
fn node_platform_and_arch() -> (&'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    (platform, arch)
}

/// Locate `copilot-sdk-<platform>-<arch>`:
///
/// 1. `DESKTOP_COPILOT_DIR`, for an explicit install;
/// 2. `copilot/` next to the executable, where a packaged build places it;
/// 3. the development tree, where pnpm keeps the optional package under
///    `node_modules/.pnpm` (or `app/node_modules` for a hoisted install).
fn find_copilot_dir(exec_path: &Path) -> Option<PathBuf> {
    let (platform, arch) = node_platform_and_arch();
    let package = format!("copilot-sdk-{platform}-{arch}");

    if let Some(dir) = std::env::var_os("DESKTOP_COPILOT_DIR").map(PathBuf::from) {
        if dir.join("prebuilds").is_dir() {
            return Some(dir);
        }
    }
    if let Some(dir) = exec_path.parent().map(|parent| parent.join("copilot")) {
        if dir.join("prebuilds").is_dir() {
            return Some(dir);
        }
    }

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..");
    let hoisted = repo
        .join("app")
        .join("node_modules")
        .join("@github")
        .join(&package);
    if hoisted.join("prebuilds").is_dir() {
        return Some(hoisted);
    }
    let store = repo.join("node_modules").join(".pnpm");
    let prefix = format!("@github+{package}@");
    std::fs::read_dir(store).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&prefix) {
            return None;
        }
        let dir = entry
            .path()
            .join("node_modules")
            .join("@github")
            .join(&package);
        dir.join("prebuilds").is_dir().then_some(dir)
    })
}
