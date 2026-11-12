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

/// Non-secret configuration used by the renderer's repository workflows.
#[derive(Clone, Debug, Deserialize)]
pub enum GitReadSettingKey {
    #[serde(rename = "core.autocrlf")]
    AutoCrlf,
    #[serde(rename = "core.safecrlf")]
    SafeCrlf,
    #[serde(rename = "pull.ff")]
    PullFf,
    #[serde(rename = "pull.rebase")]
    PullRebase,
    #[serde(rename = "trailer.separators")]
    TrailerSeparators,
    #[serde(rename = "commit.gpgsign")]
    CommitGpgSign,
}

impl GitReadSettingKey {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AutoCrlf => "core.autocrlf",
            Self::SafeCrlf => "core.safecrlf",
            Self::PullFf => "pull.ff",
            Self::PullRebase => "pull.rebase",
            Self::TrailerSeparators => "trailer.separators",
            Self::CommitGpgSign => "commit.gpgsign",
        }
    }
}

/// Read a fixed non-secret setting using Git's normal precedence and parsing.
pub async fn git_read_setting(
    path: &Path,
    key: GitReadSettingKey,
    only_local: bool,
    canonical_bool: bool,
) -> Result<Value> {
    if canonical_bool && !matches!(key, GitReadSettingKey::CommitGpgSign) {
        bail!("boolean parsing is not supported for this setting");
    }
    read_git_config(path, key.as_str(), only_local, canonical_bool).await
}

/// Read one identity field with the same precedence rules as other settings.
pub(crate) async fn git_identity_value(path: &Path, key: &str, only_local: bool) -> Result<Value> {
    if !matches!(key, "user.name" | "user.email") {
        bail!("unsupported Git identity field");
    }
    read_git_config(path, key, only_local, false).await
}

/// Resolve the identity Git would actually use, including its environment and
/// fallback rules, without exposing arbitrary `git var` variables.
pub(crate) async fn git_effective_identity(
    path: &Path,
    role: crate::GitIdentityRole,
) -> Result<Value> {
    let variable = match role {
        crate::GitIdentityRole::Author => "GIT_AUTHOR_IDENT",
        crate::GitIdentityRole::Committer => "GIT_COMMITTER_IDENT",
    };
    let output = process::run(path, &args(&["var", variable])).await?;
    if output.code == 128 {
        return Ok(Value::Null);
    }
    if output.code != 0 {
        bail!("Unable to resolve Git identity");
    }
    anyhow::ensure!(output.stdout.len() <= 65536, "Git identity exceeds 64 KiB");
    Ok(json!(String::from_utf8_lossy(&output.stdout)))
}

async fn read_git_config(
    path: &Path,
    key: &str,
    only_local: bool,
    canonical_bool: bool,
) -> Result<Value> {
    let mut command = args(&["config", "-z"]);
    if only_local {
        command.extend(args(&["--local"]));
    }
    if canonical_bool {
        command.extend(args(&["--type", "bool"]));
    }
    command.push(key.into());
    let output = process::run(path, &command).await?;
    if output.code == 1 {
        return Ok(Value::Null);
    }
    if output.code != 0 {
        bail!("Git configuration read failed");
    }
    let value = output
        .stdout
        .split(|byte| *byte == 0)
        .next()
        .unwrap_or_default();
    Ok(json!(String::from_utf8_lossy(value)))
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

/// Prepare the global configuration file and return Git's selected path.
pub async fn git_global_config_path() -> Result<String> {
    let output = process::global_config_path(&std::env::current_dir()?).await?;
    if output.code != 0 {
        bail!("Unable to prepare the global Git configuration file");
    }
    let path = String::from_utf8(output.stdout)?;
    if path.is_empty() || path.contains('\0') {
        bail!("Git returned an invalid global configuration path");
    }
    Ok(path)
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
