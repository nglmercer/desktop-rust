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

// Storage instances in the same host share one identity file. This does not
// coordinate separate host processes.
static GUID_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _guard = GUID_LOCK
            .lock()
            .map_err(|_| AppError::msg("GUID lock poisoned"))?;
        match std::fs::read_to_string(self.paths.guid_file()) {
            Ok(guid) => Ok(guid.trim().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let guid = uuid::Uuid::new_v4().to_string();
                atomic_write(&self.paths.guid_file(), guid.as_bytes())?;
                Ok(guid)
            }
            Err(e) => Err(AppError::Io(e)),
        }
    }

    pub fn save_guid(&self, guid: &str) -> Result<()> {
        let _guard = GUID_LOCK
            .lock()
            .map_err(|_| AppError::msg("GUID lock poisoned"))?;
        atomic_write(&self.paths.guid_file(), guid.as_bytes())
    }
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
    atomic_write_with(path, bytes, |source, destination| {
        std::fs::rename(source, destination)
    })
}

fn atomic_write_with(
    path: &std::path::Path,
    bytes: &[u8],
    rename: impl FnOnce(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::msg("Storage file has no parent"))?;
    let temp = parent.join(format!(".desktop-write-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // State and preferences can contain private account information.
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let written = file.write_all(bytes);
    drop(file);
    let result = written.and_then(|_| rename(&temp, path));
    // The temporary is unique and was created by this call; never remove the
    // destination when publishing fails.
    let _ = std::fs::remove_file(temp);
    result.map_err(AppError::Io)
}

#[cfg(test)]
mod guid_tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("desktop-guid-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn storage(&self) -> Storage {
            Storage::open(AppPaths {
                user_data: self.0.clone(),
                app_path: self.0.clone(),
                exec_path: self.0.join("desktop"),
                copilot_dir: None,
            })
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preserves_existing_identifiers_without_rewriting_them() {
        for existing in [
            "00112233445566778899aabbccddeeff",
            "10000000-0000-4000-8000-000000000000",
            "unexpected-existing-value",
        ] {
            let fixture = Fixture::new();
            let bytes = format!("{existing}\n");
            std::fs::write(fixture.0.join("guid"), &bytes).unwrap();
            assert_eq!(fixture.storage().get_guid().unwrap(), existing);
            assert_eq!(
                std::fs::read_to_string(fixture.0.join("guid")).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn concurrent_storage_instances_in_one_process_share_guid() {
        let fixture = Fixture::new();
        let barrier = Arc::new(Barrier::new(16));
        let values = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..16 {
                let storage = fixture.storage();
                let barrier = barrier.clone();
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    storage.get_guid().unwrap()
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(values.iter().all(|value| value == &values[0]));
        let parsed = uuid::Uuid::parse_str(&values[0]).unwrap();
        assert_eq!(parsed.get_version_num(), 4);
        assert_eq!(values[0].len(), 36);
        assert_eq!(
            std::fs::read_to_string(fixture.0.join("guid")).unwrap(),
            values[0]
        );
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn failed_save_does_not_change_observed_guid_and_retry_is_shared() {
        let fixture = Fixture::new();
        let first = fixture.storage();
        let second = fixture.storage();
        let original = first.get_guid().unwrap();
        assert_eq!(second.get_guid().unwrap(), original);
        let relocated = fixture.0.with_extension("saved");
        std::fs::rename(&fixture.0, &relocated).unwrap();
        std::fs::write(&fixture.0, b"blocks directory creation").unwrap();
        assert!(first.save_guid("replacement").is_err());
        std::fs::remove_file(&fixture.0).unwrap();
        std::fs::rename(&relocated, &fixture.0).unwrap();
        assert_eq!(first.get_guid().unwrap(), original);
        assert_eq!(second.get_guid().unwrap(), original);
        first.save_guid("replacement").unwrap();
        assert_eq!(second.get_guid().unwrap(), "replacement");
    }

    #[test]
    fn failed_publish_preserves_original_and_removes_only_owned_temporary() {
        let fixture = Fixture::new();
        let destination = fixture.0.join("guid");
        std::fs::write(&destination, b"original").unwrap();
        let other = fixture.0.join("unrelated.tmp");
        std::fs::write(&other, b"keep").unwrap();
        let result = atomic_write_with(&destination, b"replacement", |source, target| {
            assert_eq!(std::fs::read(source).unwrap(), b"replacement");
            assert_eq!(target, destination);
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected failure",
            ))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
        assert_eq!(std::fs::read(other).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
    }

    #[test]
    fn simultaneous_reads_and_save_do_not_deadlock() {
        let fixture = Fixture::new();
        let original = fixture.storage().get_guid().unwrap();
        let barrier = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            let writer = fixture.storage();
            let start = barrier.clone();
            scope.spawn(move || {
                start.wait();
                writer.save_guid("replacement").unwrap();
            });
            for _ in 0..8 {
                let reader = fixture.storage();
                let start = barrier.clone();
                let original = original.clone();
                scope.spawn(move || {
                    start.wait();
                    let seen = reader.get_guid().unwrap();
                    assert!(seen == original || seen == "replacement");
                });
            }
        });
        assert_eq!(fixture.storage().get_guid().unwrap(), "replacement");
    }

    #[test]
    fn storage_consumers_replace_files_and_keep_them_on_publish_failure() {
        let fixture = Fixture::new();
        let storage = fixture.storage();
        let mut state = AppState::default();
        storage.save_state(&state).unwrap();
        state.preferences.sidebar_width = 271;
        storage.save_state(&state).unwrap();
        assert_eq!(storage.load_state().unwrap().preferences.sidebar_width, 271);
        let mut preferences = Preferences::default();
        storage.save_preferences(&preferences).unwrap();
        preferences.sidebar_width = 272;
        storage.save_preferences(&preferences).unwrap();
        assert_eq!(storage.load_preferences().sidebar_width, 272);
        storage.set_item("key", Value::from("first")).unwrap();
        storage.set_item("key", Value::from("second")).unwrap();
        assert_eq!(
            fixture.storage().get_string("key").as_deref(),
            Some("second")
        );
        for name in ["state.json", "preferences.json", "local-storage.json"] {
            let path = fixture.0.join(name);
            let before = std::fs::read(&path).unwrap();
            assert!(
                atomic_write_with(&path, b"bad", |_, _| Err(
                    std::io::ErrorKind::PermissionDenied.into()
                ))
                .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            serde_json::from_slice::<Value>(&before).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn concurrent_replacements_never_expose_partial_file_contents() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        let first = vec![b'a'; 32 * 1024];
        let second = vec![b'b'; 32 * 1024];
        atomic_write(&path, &first).unwrap();
        let barrier = Arc::new(Barrier::new(5));
        let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(4));
        std::thread::scope(|scope| {
            for number in 0..4 {
                let path = path.clone();
                let bytes = if number % 2 == 0 {
                    first.clone()
                } else {
                    second.clone()
                };
                let barrier = barrier.clone();
                let remaining = remaining.clone();
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..16 {
                        atomic_write(&path, &bytes).unwrap();
                    }
                    remaining.fetch_sub(1, std::sync::atomic::Ordering::Release);
                });
            }
            barrier.wait();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while remaining.load(std::sync::atomic::Ordering::Acquire) != 0 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "writers did not finish"
                );
                let bytes = std::fs::read(&path).unwrap();
                assert!(bytes == first || bytes == second);
                std::thread::yield_now();
            }
        });
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes == first || bytes == second);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }
}
