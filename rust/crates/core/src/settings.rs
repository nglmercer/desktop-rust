//! Explicit Git settings. Authentication helpers/tokens never enter this API.
use crate::{args, process};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    #[default]
    Local,
    Global,
}
impl ConfigScope {
    fn flag(&self) -> &'static str {
        match self {
            Self::Local => "--local",
            Self::Global => "--global",
        }
    }
}
const KEYS: &[&str] = &[
    "user.name",
    "user.email",
    "user.signingkey",
    "commit.gpgsign",
    "gpg.format",
    "init.defaultbranch",
    "core.autocrlf",
    "core.filemode",
    "fetch.prune",
    "pull.ff",
];

/// Return the installed Git version without exposing arbitrary command execution.
pub async fn git_version() -> Result<String> {
    let output = process::run(&std::env::current_dir()?, &args(&["--version"])).await?;
    if output.code != 0 {
        bail!("Git version query failed");
    }
    let output = std::str::from_utf8(&output.stdout)?.trim();
    let version = output
        .strip_prefix("git version ")
        .ok_or_else(|| anyhow::anyhow!("unrecognized Git version"))?;
    if version.is_empty() || version.len() > 256 || version.chars().any(char::is_control) {
        bail!("unrecognized Git version");
    }
    Ok(version.to_owned())
}

/// Read one identity field using Git's effective or local-only lookup rules.
pub async fn git_identity_value(path: &Path, key: &str, only_local: bool) -> Result<Value> {
    if !matches!(key, "user.name" | "user.email") {
        bail!("unsupported Git identity field");
    }
    let mut command = args(&["config", "-z"]);
    if only_local {
        command.extend(args(&["--local"]));
    }
    command.extend(args(&["--get", key]));
    let output = process::run(path, &command).await?;
    if output.code == 1 {
        return Ok(Value::Null);
    }
    if output.code != 0 {
        bail!("{}", output.text().stderr);
    }
    let value = output
        .stdout
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default();
    Ok(json!(String::from_utf8_lossy(value)))
}

pub async fn git_settings(path: &Path, scope: ConfigScope) -> Result<Value> {
    let pattern = format!(
        "^({})$",
        KEYS.iter()
            .map(|key| key.replace('.', "\\."))
            .collect::<Vec<_>>()
            .join("|")
    );
    let output = process::run(
        path,
        &args(&["config", scope.flag(), "--null", "--get-regexp", &pattern]),
    )
    .await?;
    if !matches!(output.code, 0 | 1) {
        bail!("{}", output.text().stderr);
    }
    let mut values: BTreeMap<&str, Option<String>> = KEYS.iter().map(|key| (*key, None)).collect();
    for record in output
        .stdout
        .split(|b| *b == 0)
        .filter(|record| !record.is_empty())
    {
        if let Some(index) = record.iter().position(|b| *b == b'\n') {
            let key = std::str::from_utf8(&record[..index])?;
            if let Some(value) = values.get_mut(key) {
                *value = Some(String::from_utf8_lossy(&record[index + 1..]).into_owned());
            }
        }
    }
    Ok(json!({"scope":scope,"values":values}))
}
pub async fn set_git_setting(
    path: &Path,
    scope: ConfigScope,
    key: &str,
    value: Option<String>,
) -> Result<Value> {
    let key = key.to_ascii_lowercase();
    if !KEYS.contains(&key.as_str()) {
        bail!("unsupported Git setting");
    }
    if let Some(value) = &value {
        if value.len() > 4096 || value.contains(['\0', '\n', '\r']) {
            bail!("Git setting must be one line and at most 4096 bytes");
        }
        let allowed = match key.as_str() {
            // Git permits explicitly empty identity fields. The preferences
            // editor uses those values when a user clears a field.
            "user.name" | "user.email" => true,
            "commit.gpgsign" | "core.filemode" | "fetch.prune" => {
                matches!(value.as_str(), "true" | "false")
            }
            "core.autocrlf" => matches!(value.as_str(), "true" | "false" | "input"),
            "pull.ff" => matches!(value.as_str(), "true" | "false" | "only"),
            "gpg.format" => matches!(value.as_str(), "openpgp" | "ssh" | "x509"),
            _ => !value.trim().is_empty(),
        };
        if !allowed {
            bail!("invalid value for {key}");
        }
        if key == "init.defaultbranch" {
            let checked =
                process::run(path, &args(&["check-ref-format", "--branch", value])).await?;
            if value.starts_with('-') || value.contains("@{") || checked.code != 0 {
                bail!("invalid default branch");
            }
        }
    }
    let is_unset = value.is_none();
    let command = if let Some(value) = value {
        args(&["config", scope.flag(), "--replace-all", "--", &key, &value])
    } else {
        args(&["config", scope.flag(), "--unset-all", "--", &key])
    };
    let output = process::run(path, &command).await?;
    if output.code != 0 && !(is_unset && output.code == 5) {
        bail!("{}", output.text().stderr);
    }
    git_settings(path, scope).await
}
