//! Typed Git hook discovery. The host owns repository configuration probing and
//! executable-file inspection so renderer hook interception does not need a
//! generic Git runner or direct filesystem access.
use crate::{Repository, args, assisted_head::assisted_push_config_environment_key};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

const HOOK_NAMES: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
    "fsmonitor-watchman",
    "p4-changelist",
    "p4-prepare-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
    "post-index-change",
];

fn normalize(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                );
                if can_pop {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn resolve_repository_path(repository: &Path, value: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        value.len() <= 65536 && !value.contains('\0'),
        "Invalid hooks path"
    );
    let path = PathBuf::from(value);
    Ok(normalize(if path.is_absolute() {
        path
    } else {
        repository.join(path)
    }))
}

fn output_line(output: &[u8], what: &str) -> Result<String> {
    anyhow::ensure!(output.len() <= 65536, "{what} exceeds limit");
    let text = String::from_utf8_lossy(output);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    anyhow::ensure!(
        !text.is_empty() && !text.contains(['\n', '\r', '\0']),
        "invalid {what}"
    );
    Ok(text.to_owned())
}

fn hook_name(file_name: &str) -> Option<&'static str> {
    let without_exe = file_name.strip_suffix(".exe").unwrap_or(file_name);
    HOOK_NAMES.iter().copied().find(|hook| *hook == without_exe)
}

fn executable(path: &Path, link: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        let _ = path;
        let _ = link;
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = if link.file_type().is_symlink() {
            match std::fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(_) => return false,
            }
        } else {
            link.clone()
        };
        metadata.is_file() && metadata.mode() & 0o111 != 0
    }
}

impl Repository {
    pub(crate) async fn renderer_hooks(
        &self,
        filter: &Option<Vec<String>>,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported hook discovery environment key"
            );
        }
        if let Some(filter) = filter {
            anyhow::ensure!(filter.len() <= 256, "Invalid hook filter");
            for name in filter {
                anyhow::ensure!(
                    !name.is_empty() && name.len() <= 256 && !name.contains('\0'),
                    "Invalid hook filter"
                );
            }
        }

        let configured = self
            .run_with_environment(
                &args(&["config", "-z", "--get", "core.hooksPath"]),
                environment,
            )
            .await?;
        let path = if configured.code == 0 {
            let first = configured
                .stdout
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or_default();
            Some(String::from_utf8_lossy(first).into_owned())
        } else {
            None
        };
        let path = match path {
            Some(path) => path,
            None => {
                let fallback = self
                    .run_with_environment(&args(&["rev-parse", "--git-path", "hooks"]), environment)
                    .await?;
                anyhow::ensure!(fallback.code == 0, "Cannot resolve the hooks path");
                output_line(&fallback.stdout, "hooks path")?
            }
        };
        let path = resolve_repository_path(&self.path, &path)?;
        anyhow::ensure!(path.as_os_str().len() <= 65536, "Hooks path exceeds limit");

        let mut hooks = Vec::new();
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => return Ok(json!({"path": path.to_string_lossy(), "hooks": hooks})),
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(name) = hook_name(file_name) else {
                continue;
            };
            if filter.as_ref().is_some_and(|filter| {
                !filter.contains(&"*".to_owned()) && !filter.iter().any(|item| item == name)
            }) {
                continue;
            }
            let metadata = entry.metadata()?;
            if !(metadata.is_file() || metadata.file_type().is_symlink()) {
                continue;
            }
            if executable(&entry.path(), &metadata) {
                hooks.push(name);
            }
            anyhow::ensure!(hooks.len() <= HOOK_NAMES.len(), "Too many hooks");
        }
        hooks.sort_unstable();
        hooks.dedup();
        Ok(json!({
            "path": path.to_string_lossy(),
            "hooks": hooks,
        }))
    }
}
