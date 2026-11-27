//! Host-owned executable resolution for hook and shell discovery.

use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;

fn validate_input(command: &str, path: Option<&str>) -> Result<()> {
    anyhow::ensure!(
        !command.is_empty() && command.len() <= 4096 && !command.contains('\0'),
        "Invalid executable name"
    );
    if let Some(path) = path {
        anyhow::ensure!(
            path.len() <= 1024 * 1024 && !path.contains('\0'),
            "Invalid executable search path"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn command_candidates(command: &str) -> Vec<String> {
    let value = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
    let extensions = value
        .split(';')
        .filter(|extension| !extension.is_empty())
        .collect::<Vec<_>>();
    let extensions = if extensions.is_empty() {
        vec![".EXE", ".CMD", ".BAT", ".COM"]
    } else {
        extensions
    };
    let lower = command.to_lowercase();
    if extensions
        .iter()
        .any(|extension| lower.ends_with(&extension.to_lowercase()))
    {
        return vec![command.to_owned()];
    }
    let mut candidates = vec![command.to_owned()];
    for extension in extensions {
        let candidate = format!("{command}{extension}");
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

#[cfg(not(windows))]
fn command_candidates(command: &str) -> Vec<String> {
    vec![command.to_owned()]
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn resolve(command: &str, search_path: Option<&str>) -> Result<Option<String>> {
    validate_input(command, search_path)?;
    let has_separator = command.contains('/') || command.contains('\\');
    let mut candidates = Vec::new();
    if has_separator {
        candidates.push(PathBuf::from(command));
    } else {
        let search = search_path
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        for directory in std::env::split_paths(&search) {
            for command in command_candidates(command) {
                candidates.push(directory.join(command));
            }
        }
    }
    for candidate in candidates {
        if is_executable(&candidate)
            && let Ok(path) = candidate.into_os_string().into_string()
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Resolve the first executable matching a command in a PATH-style list.
pub async fn resolve_executable(command: String, path: Option<String>) -> Result<Value> {
    let resolved =
        crate::cancellation::blocking(move || resolve(&command, path.as_deref())).await?;
    Ok(resolved.map_or(Value::Null, Value::String))
}

#[cfg(all(test, unix))]
mod tests {
    use super::resolve;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "desktop-executable-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_the_first_executable_in_a_custom_path() {
        let fixture = Fixture::new();
        let first = fixture.0.join("first");
        let second = fixture.0.join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("tool"), "first").unwrap();
        std::fs::write(second.join("tool"), "second").unwrap();
        std::fs::set_permissions(second.join("tool"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let search = format!("{}:{}", first.display(), second.display());
        assert_eq!(
            resolve("tool", Some(&search)).unwrap(),
            Some(second.join("tool").to_string_lossy().into_owned())
        );
    }

    #[test]
    fn requires_execute_permission_and_accepts_explicit_paths() {
        let fixture = Fixture::new();
        let tool = fixture.0.join("tool");
        std::fs::write(&tool, "tool").unwrap();
        assert_eq!(
            resolve("tool", Some(fixture.0.to_str().unwrap())).unwrap(),
            None
        );
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve(tool.to_str().unwrap(), None).unwrap(),
            Some(tool.to_string_lossy().into_owned())
        );
    }
}
