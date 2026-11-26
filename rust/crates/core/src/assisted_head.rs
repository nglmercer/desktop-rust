//! Read the HEAD identity needed by assisted-commit snapshot guards.
use crate::{Repository, args};
use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

fn output_line(output: &[u8], what: &str, path: bool) -> Result<String> {
    anyhow::ensure!(output.len() <= 65536, "{what} exceeds limit");
    let text = String::from_utf8_lossy(output);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    anyhow::ensure!(
        !text.is_empty() && !text.contains('\0') && (path || !text.contains(['\n', '\r'])),
        "invalid {what}"
    );
    Ok(text.to_owned())
}

fn invalid(message: impl Into<String>) -> Value {
    json!({"kind":"invalid", "message":message.into()})
}

fn snapshot_reference_name(value: &str) -> bool {
    value.len() <= 4096
        && value.starts_with("refs/")
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        })
}

fn validate_snapshot_refs(output: &[u8]) -> Result<Vec<Value>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        output.ends_with(b"\n"),
        "reference snapshot is not terminated"
    );
    let text = std::str::from_utf8(&output[..output.len() - 1])?;
    let mut references = Vec::new();
    for row in text.split('\n') {
        let fields = row.split('\0').collect::<Vec<_>>();
        anyhow::ensure!(fields.len() == 3, "reference snapshot row is malformed");
        let [object_id, name, target] = fields.try_into().unwrap();
        anyhow::ensure!(
            matches!(object_id.len(), 40 | 64)
                && object_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "reference snapshot object ID is invalid"
        );
        anyhow::ensure!(
            snapshot_reference_name(name),
            "reference snapshot name is unsafe"
        );
        anyhow::ensure!(
            target.is_empty() || snapshot_reference_name(target),
            "reference snapshot target is unsafe"
        );
        references.push(json!({
            "object_id": object_id,
            "name": name,
            "target": if target.is_empty() { Value::Null } else { json!(target) },
        }));
    }
    Ok(references)
}

fn config_name_part(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn validate_snapshot_config(output: &[u8]) -> Result<Vec<Value>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        output.ends_with(b"\0"),
        "configuration snapshot is not terminated"
    );
    let text = std::str::from_utf8(&output[..output.len() - 1])?;
    let mut entries = Vec::new();
    for item in text.split('\0') {
        if item.is_empty() {
            continue;
        }
        let delimiter = item.find('\n');
        let key = delimiter.map_or(item, |index| &item[..index]);
        let value = delimiter.map(|index| &item[index + 1..]);
        let first_dot = key.find('.');
        let last_dot = key.rfind('.');
        anyhow::ensure!(
            first_dot.is_some() && last_dot.is_some(),
            "configuration key has no section"
        );
        let first_dot = first_dot.unwrap();
        let last_dot = last_dot.unwrap();
        let section = &key[..first_dot];
        if section.eq_ignore_ascii_case("include") || section.eq_ignore_ascii_case("includeif") {
            continue;
        }
        let subsection = (first_dot != last_dot).then(|| &key[first_dot + 1..last_dot]);
        let name = &key[last_dot + 1..];
        anyhow::ensure!(
            config_name_part(section) && config_name_part(name),
            "configuration key is invalid"
        );
        entries.push(json!({
            "section": section,
            "subsection": subsection.map_or(Value::Null, |value| json!(value)),
            "name": name,
            "value": value.map_or(Value::Null, |value| json!(value)),
        }));
    }
    Ok(entries)
}

fn validate_push_config(output: &[u8]) -> Result<Vec<Value>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        output.ends_with(b"\0"),
        "push configuration snapshot is not terminated"
    );
    let fields = output[..output.len() - 1]
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        fields.len() % 2 == 0,
        "push configuration snapshot has an incomplete entry"
    );
    let mut entries = Vec::new();
    for index in (0..fields.len()).step_by(2) {
        let origin = fields[index];
        let entry = fields[index + 1];
        anyhow::ensure!(!origin.is_empty(), "push configuration origin is empty");
        anyhow::ensure!(!entry.is_empty(), "push configuration key is empty");
        let separator = entry.iter().position(|byte| *byte == b'\n');
        let key = separator.map_or(entry, |index| &entry[..index]);
        let value = separator.map_or(&[][..], |index| &entry[index + 1..]);
        anyhow::ensure!(!key.is_empty(), "push configuration key is empty");
        entries.push(json!({
            "origin": String::from_utf8_lossy(origin),
            "key": String::from_utf8_lossy(key),
            "value": String::from_utf8_lossy(value),
        }));
    }
    Ok(entries)
}

fn validate_index_entries(entries: &[u8]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(entries.ends_with(b"\0"), "index entries are not terminated");
    for entry in entries[..entries.len() - 1].split(|byte| *byte == 0) {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            anyhow::bail!("index entry is missing its path separator");
        };
        let metadata = &entry[..tab];
        let path = &entry[tab + 1..];
        anyhow::ensure!(!path.is_empty(), "index entry has an empty path");
        let fields = metadata.split(|byte| *byte == b' ').collect::<Vec<_>>();
        anyhow::ensure!(fields.len() == 3, "index entry metadata is invalid");
        anyhow::ensure!(
            fields[0].len() == 6 && fields[0].iter().all(|byte| matches!(byte, b'0'..=b'7')),
            "index entry mode is invalid"
        );
        anyhow::ensure!(
            matches!(fields[1].len(), 40 | 64)
                && fields[1].iter().all(|byte| byte.is_ascii_hexdigit()),
            "index entry object ID is invalid"
        );
        anyhow::ensure!(
            fields[2].len() == 1 && matches!(fields[2][0], b'0'..=b'3'),
            "index entry stage is invalid"
        );
    }
    Ok(())
}

fn parse_config_values(output: &[u8]) -> Result<Vec<Value>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        output.ends_with(b"\0"),
        "configuration values are not terminated"
    );
    Ok(output[..output.len() - 1]
        .split(|byte| *byte == 0)
        .map(|value| json!(String::from_utf8_lossy(value)))
        .collect())
}

fn assisted_push_config_environment_key(key: &str) -> bool {
    matches!(
        key,
        "LOCAL_GIT_DIRECTORY"
            | "GIT_CONFIG"
            | "GIT_DIR"
            | "GIT_COMMON_DIR"
            | "GIT_WORK_TREE"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_INDEX_FILE"
            | "GIT_SHALLOW_FILE"
            | "GIT_GRAFT_FILE"
            | "GIT_REPLACE_REF_BASE"
            | "GIT_NO_REPLACE_OBJECTS"
            | "GIT_NAMESPACE"
            | "GIT_CONFIG_SYSTEM"
            | "GIT_CONFIG_GLOBAL"
            | "GIT_CONFIG_NOSYSTEM"
            | "GIT_CONFIG_PARAMETERS"
            | "GIT_CONFIG_COUNT"
            | "HOME"
            | "XDG_CONFIG_HOME"
    ) || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
}

impl Repository {
    pub(crate) async fn assisted_commit_config(&self) -> Result<Value> {
        let output = crate::process::run_without_git_config(
            &self.path,
            &args(&["config", "--null", "--list", "--includes"]),
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot capture Git configuration");
        Ok(json!({"entries":validate_snapshot_config(&output.stdout)?}))
    }

    pub(crate) async fn assisted_commit_push_config(
        &self,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported push configuration environment key"
            );
        }
        let output = crate::process::run_without_git_config_with_environment(
            &self.path,
            &args(&["config", "--null", "--list", "--show-origin", "--includes"]),
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot capture Git push configuration");
        Ok(json!({"entries":validate_push_config(&output.stdout)?}))
    }

    pub(crate) async fn assisted_commit_config_values(
        &self,
        key: &str,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !key.is_empty() && key.len() <= 4096 && !key.contains('\0'),
            "invalid configuration key"
        );
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported push configuration environment key"
            );
        }
        let output = crate::process::run_without_git_config_with_environment(
            &self.path,
            &args(&["config", "--null", "--get-all", "--", key]),
            environment,
        )
        .await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "Cannot read Git configuration values"
        );
        let values = if output.code == 1 {
            Vec::new()
        } else {
            parse_config_values(&output.stdout)?
        };
        Ok(json!({"values":values}))
    }

    pub(crate) async fn assisted_commit_remote_push_urls(
        &self,
        remote: &str,
        all: bool,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !remote.is_empty() && remote.len() <= 4096 && !remote.contains('\0'),
            "invalid remote name"
        );
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported push configuration environment key"
            );
        }
        let mut command_args = vec!["remote", "get-url", "--push"];
        if all {
            command_args.push("--all");
        }
        command_args.push("--");
        command_args.push(remote);
        let output = crate::process::run_without_git_config_with_environment(
            &self.path,
            &args(&command_args),
            environment,
        )
        .await?;
        anyhow::ensure!(matches!(output.code, 0 | 2), "Cannot read remote push URL");
        Ok(json!({
            "exit_code": output.code,
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }

    pub(crate) async fn assisted_commit_config_path(
        &self,
        variable: &str,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        anyhow::ensure!(
            matches!(variable, "GIT_CONFIG_SYSTEM" | "GIT_CONFIG_GLOBAL"),
            "unsupported configuration path variable"
        );
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported push configuration environment key"
            );
        }
        let output = crate::process::run_without_git_config_with_environment(
            &self.path,
            &args(&["var", variable]),
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot read Git configuration path");
        anyhow::ensure!(
            output.stdout.len() <= 65536,
            "configuration path is too large"
        );
        Ok(json!({"stdout":String::from_utf8_lossy(&output.stdout)}))
    }

    pub(crate) async fn assisted_commit_ref_format(&self, reference: &str) -> Result<Value> {
        anyhow::ensure!(
            !reference.is_empty() && reference.len() <= 4096 && !reference.contains('\0'),
            "invalid reference name"
        );
        let output = self.run(&args(&["check-ref-format", reference])).await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "Cannot validate reference format"
        );
        Ok(json!({"valid":output.code == 0}))
    }

    pub(crate) async fn assisted_commit_staged_config_values(
        &self,
        key: &str,
        snapshot_id: &str,
        size: u64,
        environment: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        anyhow::ensure!(
            !key.is_empty() && key.len() <= 4096 && !key.contains('\0'),
            "invalid configuration key"
        );
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_push_config_environment_key(key),
                "unsupported push configuration environment key"
            );
        }
        let snapshot_id = uuid::Uuid::parse_str(snapshot_id)?;
        let (input, _permit) =
            crate::comparison_snapshots::take_upload(&self.id(), snapshot_id, size)?;
        let output = crate::process::run_without_git_config_with_environment_input(
            &self.path,
            &args(&[
                "config",
                "--no-includes",
                "--null",
                "--file",
                "-",
                "--get-all",
                "--",
                key,
            ]),
            Some(input),
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot validate staged Git configuration");
        Ok(json!({"values":parse_config_values(&output.stdout)?}))
    }

    pub(crate) async fn assisted_commit_refs(&self) -> Result<Value> {
        let output = self
            .run(&args(&[
                "for-each-ref",
                "--format=%(objectname)%00%(refname)%00%(symref)",
            ]))
            .await?;
        anyhow::ensure!(output.code == 0, "Cannot capture repository references");
        Ok(json!({"refs":validate_snapshot_refs(&output.stdout)?}))
    }

    /// Capture the exact stage entries needed to reproduce the real index in
    /// assisted-commit's temporary attributes index.
    pub(crate) async fn assisted_commit_index_entries(&self) -> Result<Value> {
        let output = self.run(&args(&["ls-files", "--stage", "-z"])).await?;
        anyhow::ensure!(output.code == 0, "Cannot capture index entries");
        validate_index_entries(&output.stdout)?;
        Ok(json!({"entries_base64":STANDARD.encode(&output.stdout)}))
    }

    pub(crate) async fn assisted_commit_git_path(&self, git_path: &str) -> Result<Value> {
        anyhow::ensure!(
            !git_path.is_empty()
                && git_path.len() <= 4096
                && !git_path.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
                && !std::path::Path::new(git_path).is_absolute()
                && !git_path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == ".."),
            "invalid assisted-commit Git path"
        );
        let output = self
            .run(&args(&[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                git_path,
            ]))
            .await?;
        anyhow::ensure!(output.code == 0, "Cannot resolve the Git path");
        let path = output_line(&output.stdout, "Git path", true)?;
        anyhow::ensure!(
            std::path::Path::new(&path).is_absolute(),
            "Git path must be absolute"
        );
        Ok(json!({"path":path}))
    }

    pub(crate) async fn assisted_commit_safety(&self) -> Result<Value> {
        let replacements = self.run(&args(&["replace", "--list"])).await?;
        anyhow::ensure!(replacements.code == 0, "Cannot inspect replacement refs");
        if !replacements.stdout.is_empty() {
            return Ok(invalid(
                "Assisted commits do not support replacement refs or grafted history",
            ));
        }
        let directory = self.git_directory().await?;
        let grafts = std::env::var_os("GIT_GRAFT_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| directory.join("info/grafts"));
        match std::fs::symlink_metadata(&grafts) {
            Ok(_) => {
                return Ok(invalid(
                    "Assisted commits do not support replacement refs or grafted history",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for name in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
            "BISECT_LOG",
        ] {
            match std::fs::symlink_metadata(directory.join(name)) {
                Ok(_) => {
                    return Ok(invalid(format!(
                        "Cannot create assisted commits during a Git operation ({name})"
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let conflicts = self.run(&args(&["ls-files", "--unmerged", "-z"])).await?;
        anyhow::ensure!(
            conflicts.code == 0,
            "Cannot inspect unresolved index entries"
        );
        if !conflicts.stdout.is_empty() {
            return Ok(invalid("The real index contains unresolved conflicts"));
        }
        Ok(json!({"kind":"ok"}))
    }

    /// Read HEAD, its optional branch, tip, and absolute Git directory in one
    /// repository operation. Expected unsafe states are data, not transport errors.
    pub(crate) async fn assisted_commit_head(&self) -> Result<Value> {
        let directory = self
            .run(&args(&["rev-parse", "--absolute-git-dir"]))
            .await?;
        anyhow::ensure!(directory.code == 0, "Cannot read the Git directory");
        let git_dir = output_line(&directory.stdout, "Git directory", true)?;
        anyhow::ensure!(
            std::path::Path::new(&git_dir).is_absolute(),
            "Git directory must be absolute"
        );
        let top = self.run(&args(&["rev-parse", "--show-toplevel"])).await?;
        anyhow::ensure!(top.code == 0, "Cannot read the worktree root");
        let worktree_root = output_line(&top.stdout, "worktree root", true)?;
        let format = self.run(&args(&["rev-parse", "--show-ref-format"])).await?;
        anyhow::ensure!(format.code == 0, "Cannot read the ref format");
        let ref_format = output_line(&format.stdout, "ref format", false)?;
        match std::fs::symlink_metadata(std::path::Path::new(&git_dir).join("HEAD")) {
            Ok(metadata) if !metadata.is_file() => {
                return Ok(invalid(
                    "Assisted commits require a regular HEAD file; symlink HEAD is not supported",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let symbolic = self
            .run(&args(&["symbolic-ref", "--quiet", "--no-recurse", "HEAD"]))
            .await?;
        anyhow::ensure!(matches!(symbolic.code, 0 | 1), "Cannot read symbolic HEAD");
        let reference = if symbolic.code == 0 {
            Some(output_line(&symbolic.stdout, "symbolic HEAD", false)?)
        } else {
            None
        };
        if let Some(reference) = &reference {
            if !reference.starts_with("refs/heads/") {
                return Ok(invalid(
                    "Assisted commits require a branch or a detached commit",
                ));
            }
            let alias = self
                .run(&args(&[
                    "symbolic-ref",
                    "--quiet",
                    "--no-recurse",
                    reference,
                ]))
                .await?;
            anyhow::ensure!(
                matches!(alias.code, 0 | 1),
                "Cannot read branch symbolic ref"
            );
            if alias.code == 0 {
                return Ok(invalid(
                    "Assisted commits do not support symbolic branch aliases",
                ));
            }
        }

        let tip = self
            .run(&args(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                "HEAD^{commit}",
            ]))
            .await?;
        anyhow::ensure!(matches!(tip.code, 0 | 1), "Cannot read HEAD commit");
        let sha = if tip.code == 0 {
            Some(output_line(&tip.stdout, "HEAD commit", false)?)
        } else {
            None
        };
        if sha.as_deref().is_some_and(|value| {
            !matches!(value.len(), 40 | 64)
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Ok(invalid("HEAD is not a full object ID"));
        }
        if reference.is_none() && sha.is_none() {
            return Ok(invalid("An unborn detached HEAD cannot be committed"));
        }
        Ok(json!({
            "kind":"ok",
            "git_dir":git_dir,
            "worktree_root":worktree_root,
            "ref_format":ref_format,
            "ref":reference,
            "sha":sha,
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::Repository;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{fs, process::Command};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("desktop-assisted-head-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let fixture = Self(root);
            fixture.git(&["init", "-q", "-b", "main"]);
            fixture.git(&["config", "user.name", "Test"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fixture
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().into()
        }

        fn commit(&self) -> String {
            fs::write(self.0.join("file"), "content\n").unwrap();
            self.git(&["add", "file"]);
            self.git(&["commit", "-qm", "commit"]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn safety_detects_grafts_operations_and_conflicts() {
        let fixture = Fixture::new();
        let head = fixture.commit();
        let repository = Repository::open(&fixture.0).await.unwrap();
        assert_eq!(
            repository.assisted_commit_safety().await.unwrap()["kind"],
            "ok"
        );
        let entries = repository.assisted_commit_index_entries().await.unwrap();
        let entries = STANDARD
            .decode(entries["entries_base64"].as_str().unwrap())
            .unwrap();
        assert!(entries.ends_with(b"\tfile\0"));
        assert!(entries.starts_with(b"100644 "));
        let refs = repository.assisted_commit_refs().await.unwrap();
        assert!(refs["refs"].as_array().unwrap().iter().any(|reference| {
            reference["object_id"] == head
                && reference["name"] == "refs/heads/main"
                && reference["target"].is_null()
        }));
        fixture.git(&["config", "branch.main.remote", "origin"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ]);
        let config = repository.assisted_commit_config().await.unwrap();
        assert!(config["entries"].as_array().unwrap().iter().any(|entry| {
            entry["section"] == "branch"
                && entry["subsection"] == "main"
                && entry["name"] == "remote"
                && entry["value"] == "origin"
        }));
        let push_config = repository
            .assisted_commit_push_config(&Default::default())
            .await
            .unwrap();
        assert!(
            push_config["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| {
                    entry["origin"].as_str().unwrap().starts_with("file:")
                        && entry["key"] == "branch.main.remote"
                        && entry["value"] == "origin"
                })
        );
        let global = fixture.0.join("push-global-config");
        fs::write(&global, "[push]\n\trequiresNative = true\n").unwrap();
        let environment = std::collections::BTreeMap::from([
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                Some(global.to_string_lossy().into_owned()),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), Some("1".to_string())),
        ]);
        let scoped = repository
            .assisted_commit_push_config(&environment)
            .await
            .unwrap();
        assert!(scoped["entries"].as_array().unwrap().iter().any(|entry| {
            entry["origin"]
                .as_str()
                .unwrap()
                .ends_with("push-global-config")
                && entry["key"] == "push.requiresnative"
                && entry["value"] == "true"
        }));
        let global_path = repository
            .assisted_commit_config_path("GIT_CONFIG_GLOBAL", &environment)
            .await
            .unwrap();
        assert_eq!(
            global_path["stdout"],
            format!("{}\n", global.to_string_lossy())
        );
        let unsupported =
            std::collections::BTreeMap::from([("GIT_TRACE".to_string(), Some("1".to_string()))]);
        assert!(
            repository
                .assisted_commit_push_config(&unsupported)
                .await
                .is_err()
        );
        let push_url = repository
            .assisted_commit_remote_push_urls("origin", false, &Default::default())
            .await
            .unwrap();
        assert_eq!(push_url["exit_code"], 0);
        assert_eq!(push_url["stdout"], "https://example.invalid/repo.git\n");
        let push_urls = repository
            .assisted_commit_remote_push_urls("origin", true, &Default::default())
            .await
            .unwrap();
        assert_eq!(push_urls["stdout"], push_url["stdout"]);
        assert!(
            repository
                .assisted_commit_ref_format("refs/heads/main")
                .await
                .unwrap()["valid"]
                .as_bool()
                .unwrap()
        );
        assert!(
            !repository
                .assisted_commit_ref_format("refs/heads/bad..name")
                .await
                .unwrap()["valid"]
                .as_bool()
                .unwrap()
        );
        let values = repository
            .assisted_commit_config_values("branch.main.remote", &Default::default())
            .await
            .unwrap();
        assert_eq!(values["values"][0], "origin");
        let missing = repository
            .assisted_commit_config_values("branch.main.missing", &Default::default())
            .await
            .unwrap();
        assert!(missing["values"].as_array().unwrap().is_empty());
        let index = repository.assisted_commit_git_path("index").await.unwrap();
        assert!(index["path"].as_str().unwrap().ends_with(".git/index"));
        assert!(
            repository
                .assisted_commit_git_path("../outside")
                .await
                .is_err()
        );
        let grafts = fixture.0.join(".git/info/grafts");
        fs::write(&grafts, "0000000000000000000000000000000000000000\n").unwrap();
        let grafted = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(grafted["kind"], "invalid");
        assert!(
            grafted["message"]
                .as_str()
                .unwrap()
                .contains("grafted history")
        );
        fs::remove_file(&grafts).unwrap();
        let merge_head = fixture.0.join(".git/MERGE_HEAD");
        fs::write(&merge_head, "0000000000000000000000000000000000000000\n").unwrap();
        let merging = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(merging["kind"], "invalid");
        assert!(merging["message"].as_str().unwrap().contains("MERGE_HEAD"));
        fs::remove_file(&merge_head).unwrap();
        let blob = fixture.git(&["hash-object", "-w", "file"]);
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&fixture.0)
            .args(["update-index", "--index-info"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        write!(
            child.stdin.as_mut().unwrap(),
            "100644 {blob} 1\tfile\n100644 {blob} 2\tfile\n"
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        let conflicted = repository.assisted_commit_safety().await.unwrap();
        assert_eq!(conflicted["kind"], "invalid");
        assert!(
            conflicted["message"]
                .as_str()
                .unwrap()
                .contains("unresolved conflicts")
        );
    }

    #[tokio::test]
    async fn staged_config_values_use_the_pinned_input() {
        let fixture = Fixture::new();
        let repository = Repository::open(&fixture.0).await.unwrap();
        let owner = crate::comparison_snapshots::Owner::new();
        crate::comparison_snapshots::scope(owner.clone(), async {
            let permit = crate::comparison_snapshots::reserve().await.unwrap();
            let upload =
                crate::comparison_snapshots::insert_upload(&repository.id(), permit).unwrap();
            let id = upload["id"].as_str().unwrap().to_owned();
            let uuid = uuid::Uuid::parse_str(&id).unwrap();
            let bytes = "[user]\n\temail = test@example.com\n";
            let input = STANDARD.encode(bytes);
            crate::comparison_snapshots::append_upload(&repository.id(), uuid, 0, &input).unwrap();
            let values = repository
                .assisted_commit_staged_config_values(
                    "user.email",
                    &id,
                    bytes.len() as u64,
                    &Default::default(),
                )
                .await
                .unwrap();
            assert_eq!(values["values"][0], "test@example.com");
        })
        .await;
        owner.close();
    }

    #[tokio::test]
    async fn reads_attached_detached_and_unborn_head() {
        let fixture = Fixture::new();
        let head = fixture.commit();
        let repository = Repository::open(&fixture.0).await.unwrap();
        let attached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(attached["kind"], "ok");
        assert_eq!(attached["ref"], "refs/heads/main");
        assert_eq!(attached["sha"], head);
        assert!(attached["git_dir"].as_str().unwrap().ends_with(".git"));
        assert_eq!(
            attached["worktree_root"],
            fixture.0.to_string_lossy().as_ref()
        );
        assert_eq!(attached["ref_format"], "files");

        fixture.git(&["switch", "-q", "--detach", "HEAD"]);
        let detached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(detached["ref"], serde_json::Value::Null);
        assert_eq!(detached["sha"], head);

        fixture.git(&["switch", "-q", "--orphan", "unborn"]);
        let unborn = repository.assisted_commit_head().await.unwrap();
        assert_eq!(unborn["kind"], "ok");
        assert_eq!(unborn["ref"], "refs/heads/unborn");
        assert_eq!(unborn["sha"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn reports_branch_aliases_and_unborn_detached_head_as_invalid() {
        let fixture = Fixture::new();
        fixture.commit();
        fixture.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/main"]);
        fixture.git(&["switch", "-q", "alias"]);
        let repository = Repository::open(&fixture.0).await.unwrap();
        let aliased = repository.assisted_commit_head().await.unwrap();
        assert_eq!(aliased["kind"], "invalid");
        assert!(
            aliased["message"]
                .as_str()
                .unwrap()
                .contains("symbolic branch aliases")
        );

        fs::write(
            fixture.0.join(".git/HEAD"),
            "0000000000000000000000000000000000000000\n",
        )
        .unwrap();
        let unborn_detached = repository.assisted_commit_head().await.unwrap();
        assert_eq!(unborn_detached["kind"], "invalid");
        assert!(
            unborn_detached["message"]
                .as_str()
                .unwrap()
                .contains("unborn detached HEAD")
        );
    }

    #[tokio::test]
    async fn rejects_symlinked_head_as_invalid() {
        let fixture = Fixture::new();
        fixture.commit();
        #[cfg(unix)]
        {
            let head = fixture.0.join(".git/HEAD");
            let original = fs::read(&head).unwrap();
            fs::remove_file(&head).unwrap();
            std::os::unix::fs::symlink("refs/heads/main", &head).unwrap();
            let repository = Repository::open(&fixture.0).await.unwrap();
            let value = repository.assisted_commit_head().await.unwrap();
            assert_eq!(value["kind"], "invalid");
            assert!(value["message"].as_str().unwrap().contains("regular HEAD"));
            fs::remove_file(&head).unwrap();
            fs::write(&head, original).unwrap();
        }
    }
}
