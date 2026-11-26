//! Typed read-only object access for the assisted-commit private object view.
use crate::{
    GitPath, Repository, args, assisted_head::assisted_push_config_environment_key, validate_path,
};
use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;

fn object_id(value: &str, what: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid {what}"
    );
    Ok(())
}

fn output_line(output: &[u8], what: &str) -> Result<String> {
    anyhow::ensure!(output.len() <= 4096, "{what} exceeds limit");
    let text = String::from_utf8_lossy(output);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    anyhow::ensure!(
        !text.is_empty() && !text.contains(['\n', '\r', '\0']),
        "invalid {what}"
    );
    Ok(text.to_owned())
}

fn validate_environment(environment: &BTreeMap<String, Option<String>>) -> Result<()> {
    for key in environment.keys() {
        anyhow::ensure!(
            assisted_object_environment_key(key),
            "unsupported assisted object environment key"
        );
    }
    Ok(())
}

fn assisted_object_environment_key(key: &str) -> bool {
    assisted_push_config_environment_key(key) || key == "GIT_LITERAL_PATHSPECS"
}

fn assisted_diff_environment_key(key: &str) -> bool {
    assisted_object_environment_key(key) || matches!(key, "GIT_ATTR_NOSYSTEM" | "GIT_ATTR_SOURCE")
}

fn unsupported() -> Value {
    json!({"kind":"unsupported"})
}

impl Repository {
    /// Read one exact blob entry from a private tree. The host parses Git's
    /// NUL-terminated record and the renderer retains the legacy product error
    /// for trees, submodules, or unexpected records.
    pub(crate) async fn assisted_commit_tree_entry(
        &self,
        tree: &str,
        path: GitPath,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        object_id(tree, "tree object")?;
        validate_environment(environment)?;
        let path = validate_path(path)?;
        anyhow::ensure!(
            !Path::new(&path).components().any(|component| component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(".git")),
            "invalid selected path"
        );
        let mut command = args(&["ls-tree", "-z", "--full-tree", tree, "--"]);
        command.push(path.clone());
        let output = self.run_with_environment(&command, environment).await?;
        anyhow::ensure!(output.code == 0, "Cannot read assisted tree entry");
        let records = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .collect::<Vec<_>>();
        if records.is_empty() {
            return Ok(json!({"kind":"missing"}));
        }
        if records.len() != 1 {
            return Ok(unsupported());
        }
        let record = records[0];
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return Ok(unsupported());
        };
        if &record[tab + 1..] != path.as_encoded_bytes() {
            return Ok(unsupported());
        }
        let fields = record[..tab]
            .split(|byte| *byte == b' ')
            .collect::<Vec<_>>();
        if fields.len() != 3 || fields[1] != b"blob" {
            return Ok(unsupported());
        }
        let mode = std::str::from_utf8(fields[0]);
        let sha = std::str::from_utf8(fields[2]);
        let (Ok(mode), Ok(sha)) = (mode, sha) else {
            return Ok(unsupported());
        };
        if mode.len() != 6
            || !mode.bytes().all(|byte| matches!(byte, b'0'..=b'7'))
            || object_id(sha, "blob object").is_err()
        {
            return Ok(unsupported());
        }
        Ok(json!({"kind":"entry","mode":mode,"sha":sha}))
    }

    /// Resolve a revision to its tree inside the private object view.
    pub(crate) async fn assisted_commit_tree_revision(
        &self,
        revision: &str,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !revision.is_empty() && revision.len() <= 4096 && !revision.contains('\0'),
            "invalid assisted tree revision"
        );
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_diff_environment_key(key),
                "unsupported assisted revision environment key"
            );
        }
        let peeled = format!("{revision}^{{tree}}");
        let output = crate::process::run_git_with_environment(
            &self.path,
            &args(&["rev-parse", "--verify", "--end-of-options", &peeled]),
            None,
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot resolve assisted tree revision");
        let tree = output_line(&output.stdout, "tree object")?;
        object_id(&tree, "tree object")?;
        Ok(json!({"tree":tree}))
    }

    /// Read the private capture configuration that controls modes, symlinks, and filters.
    pub(crate) async fn assisted_commit_capture_config(
        &self,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_diff_environment_key(key),
                "unsupported assisted capture environment key"
            );
        }
        let boolean = async |key: &str| -> Result<Value> {
            let output = crate::process::run_git_with_environment(
                &self.path,
                &args(&["config", "--bool", "--get", key]),
                None,
                None,
                environment,
            )
            .await?;
            anyhow::ensure!(
                matches!(output.code, 0 | 1),
                "Cannot read capture boolean config"
            );
            if output.code == 1 {
                return Ok(Value::Null);
            }
            let value = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
            match value {
                b"true" => Ok(Value::Bool(true)),
                b"false" => Ok(Value::Bool(false)),
                _ => anyhow::bail!("capture boolean config is malformed"),
            }
        };
        let filters = crate::process::run_git_with_environment(
            &self.path,
            &args(&[
                "config",
                "--null",
                "--get-regexp",
                "^filter\\..*\\.(clean|process)$",
            ]),
            None,
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(
            matches!(filters.code, 0 | 1),
            "Cannot read capture filter config"
        );
        anyhow::ensure!(
            filters.stdout.len() <= 2 * 1024 * 1024,
            "capture filter config exceeds limit"
        );
        Ok(json!({
            "filemode": boolean("core.filemode").await?,
            "symlinks": boolean("core.symlinks").await?,
            "filter_output": String::from_utf8_lossy(&filters.stdout),
        }))
    }

    /// Determine whether a partial diff path is affected by a text conversion driver.
    pub(crate) async fn assisted_commit_diff_driver(
        &self,
        path: GitPath,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_diff_environment_key(key),
                "unsupported assisted diff environment key"
            );
        }
        let path = validate_path(path)?;
        let mut input = path.as_encoded_bytes().to_vec();
        input.push(0);
        let output = crate::process::run_git_with_environment(
            &self.path,
            &args(&["check-attr", "-z", "--stdin", "diff"]),
            Some(input),
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot inspect diff attributes");
        let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
        anyhow::ensure!(
            fields.len() == 4
                && fields[3].is_empty()
                && fields[0] == path.as_encoded_bytes()
                && fields[1] == b"diff",
            "diff attribute output is malformed"
        );
        let attribute = String::from_utf8_lossy(fields[2]).into_owned();
        let textconv_exists = async |driver: &str| -> Result<bool> {
            let key = format!("diff.{driver}.textconv");
            let output = crate::process::run_git_with_environment(
                &self.path,
                &args(&["config", "--get", &key]),
                None,
                None,
                environment,
            )
            .await?;
            anyhow::ensure!(matches!(output.code, 0 | 1), "Cannot inspect diff textconv");
            Ok(output.code == 0)
        };
        if matches!(attribute.as_str(), "set" | "unset" | "unspecified")
            && textconv_exists(&attribute).await?
        {
            return Ok(json!({"kind":"ambiguous-sentinel"}));
        }
        let driver = if attribute == "unspecified" || attribute == "set" {
            "default"
        } else {
            attribute.as_str()
        };
        if textconv_exists(driver).await? {
            return Ok(json!({"kind":"textconv"}));
        }
        if driver != "default" && textconv_exists("default").await? {
            return Ok(json!({"kind":"fallback-textconv"}));
        }
        Ok(json!({"kind":"safe"}))
    }

    /// Read an exact blob from the private object view. Blob bytes stay inside
    /// the typed host boundary and are returned as bounded base64 because the
    /// legacy renderer consumes the complete byte string for comparison and
    /// index reconstruction.
    pub(crate) async fn assisted_commit_blob(
        &self,
        sha: &str,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        object_id(sha, "blob object")?;
        validate_environment(environment)?;
        let output = crate::process::run_git_with_environment_limit(
            &self.path,
            &args(&["cat-file", "blob", sha]),
            None,
            None,
            environment,
            MAX_BLOB_BYTES,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot read assisted blob");
        Ok(json!({"bytes_base64":STANDARD.encode(output.stdout)}))
    }
}
