//! Replaces Dexie/IndexedDB (`app/src/lib/databases/*`) and the renderer's
//! `localStorage` (`app/src/lib/local-storage.ts`) with a single atomic JSON
//! document plus a localStorage-shaped key/value table.
//!
//! Writes go to a temp file followed by a rename, so an interrupted write can
//! never leave a half-written state file — the same guarantee Dexie gave us.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::error::{AppError, Result};
use crate::models::app_state::{AppState, Preferences};

use super::paths::AppPaths;

#[derive(Debug)]
pub struct Storage {
    paths: AppPaths,
    kv: std::sync::Mutex<HashMap<String, Value>>,
}

impl Storage {
    pub fn open(paths: AppPaths) -> Result<Self> {
        let kv = read_json(&paths.key_value_file()).unwrap_or_else(|_| HashMap::new());
        Ok(Self {
            paths,
            kv: std::sync::Mutex::new(kv),
        })
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn load_state(&self) -> Result<AppState> {
        match read_json::<AppState>(&self.paths.state_file()) {
            Ok(mut state) => {
                if state.version != crate::models::app_state::STATE_VERSION {
                    tracing::warn!(
                        "state.json was written by migration version {}; current is {}",
                        state.version,
                        crate::models::app_state::STATE_VERSION
                    );
                    state.version = crate::models::app_state::STATE_VERSION;
                }
                Ok(state)
            }
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(AppState::default())
            }
            Err(e) => {
                tracing::warn!("could not read state.json, starting fresh: {e}");
                Ok(AppState::default())
            }
        }
    }

    pub fn save_state(&self, state: &AppState) -> Result<()> {
        write_json(&self.paths.state_file(), state)
    }

    pub fn load_preferences(&self) -> Preferences {
        read_json(&self.paths.preferences_file()).unwrap_or_default()
    }

    pub fn save_preferences(&self, preferences: &Preferences) -> Result<()> {
        write_json(&self.paths.preferences_file(), preferences)
    }

    /// localStorage-shaped API used by the ~40 preference helpers in
    /// `app-store.ts`. Values are arbitrary JSON, exactly like `setObject`.
    pub fn get_item(&self, key: &str) -> Option<Value> {
        self.kv.lock().ok()?.get(key).cloned()
    }

    pub fn set_item(&self, key: &str, value: Value) -> Result<()> {
        {
            let mut guard = self
                .kv
                .lock()
                .map_err(|_| AppError::msg("local-storage lock poisoned"))?;
            guard.insert(key.to_string(), value);
        }
        self.flush_kv()
    }

    pub fn get_boolean(&self, key: &str, default: bool) -> bool {
        self.get_item(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    pub fn get_number(&self, key: &str, default: f64) -> f64 {
        self.get_item(key)
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.get_item(key)
            .and_then(|v| v.as_str().map(str::to_string))
    }

    pub fn get_string_array(&self, key: &str) -> Vec<String> {
        self.get_item(key)
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default()
    }

    fn flush_kv(&self) -> Result<()> {
        let snapshot = self
            .kv
            .lock()
            .map_err(|_| AppError::msg("local-storage lock poisoned"))?
            .clone();
        write_json(&self.paths.key_value_file(), &snapshot)
    }

    pub fn get_guid(&self) -> Result<String> {
        match std::fs::read_to_string(self.paths.guid_file()) {
            Ok(guid) => Ok(guid.trim().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let guid = generate_guid();
                self.save_guid(&guid)?;
                Ok(guid)
            }
            Err(e) => Err(AppError::Io(e)),
        }
    }

    pub fn save_guid(&self, guid: &str) -> Result<()> {
        atomic_write(&self.paths.guid_file(), guid.as_bytes())
    }
}

fn generate_guid() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let hash = crate::models::repository::simple_hash(&format!("{now}:{}", std::process::id()));
    format!("{hash:016x}{:016x}", now as u64)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)
}

pub fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    // `rename` replaces atomically on Windows too when the target is a file.
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows rename-over-existing can fail under some AV products.
            let _ = std::fs::remove_file(path);
            std::fs::rename(&temp, path)?;
            Ok(())
        }
    }
}
