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

struct TemporaryIndexDirectory {
    path: std::path::PathBuf,
}

impl TemporaryIndexDirectory {
    fn create() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("desktop-assisted-index-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path })
    }
}

impl Drop for TemporaryIndexDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn validate_index_tree_path(path: &GitPath) -> Result<std::ffi::OsString> {
    let path = validate_path(path.clone())?;
    anyhow::ensure!(
        !Path::new(&path).components().any(|component| component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".git")),
        "invalid selected path"
    );
    Ok(path)
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

    /// Write one blob through the private object view, applying exactly the
    /// frozen path filters when the renderer supplies a repository-relative path.
    pub(crate) async fn assisted_commit_hash_blob(
        &self,
        bytes_base64: &str,
        path: Option<GitPath>,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_diff_environment_key(key),
                "unsupported assisted hash environment key"
            );
        }
        anyhow::ensure!(
            bytes_base64.len() <= 768 * 1024 * 1024,
            "assisted blob input is too large"
        );
        let bytes = STANDARD.decode(bytes_base64)?;
        anyhow::ensure!(
            bytes.len() <= MAX_BLOB_BYTES as usize,
            "assisted blob input is too large"
        );
        let mut command = args(&["hash-object", "-w", "--stdin"]);
        if let Some(path) = path {
            let path = validate_index_tree_path(&path)?;
            let mut argument = std::ffi::OsString::from("--path=");
            argument.push(path);
            command.push(argument);
        } else {
            command.push("--no-filters".into());
        }
        let output = crate::process::run_git_with_environment(
            &self.path,
            &command,
            Some(bytes),
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot write assisted blob");
        let sha = output_line(&output.stdout, "written blob object")?;
        object_id(&sha, "written blob object")?;
        Ok(json!({"sha":sha}))
    }

    /// Verify a commit object created in the private object view. The renderer
    /// retains exact legacy validation/reporting; the host fixes every command
    /// shape and returns bounded byte-preserving evidence.
    pub(crate) async fn assisted_commit_verify_created_commit(
        &self,
        sha: &str,
        parent: Option<&str>,
        tree: &str,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        object_id(sha, "created commit")?;
        if let Some(parent) = parent {
            object_id(parent, "created commit parent")?;
        }
        object_id(tree, "planned tree")?;
        validate_environment(environment)?;
        let git_dir = environment
            .get("GIT_DIR")
            .and_then(|value| value.as_deref())
            .ok_or_else(|| anyhow::anyhow!("private commit verification requires GIT_DIR"))?;
        anyhow::ensure!(
            Path::new(git_dir).is_absolute() && git_dir.len() <= 65536,
            "invalid private Git directory"
        );

        let replacements = crate::process::run_git_with_environment(
            &self.path,
            &args(&["replace", "--list"]),
            None,
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(
            replacements.code == 0,
            "Cannot verify private replacement state"
        );
        let grafts = match std::fs::metadata(Path::new(git_dir).join("info").join("grafts")) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let object = crate::process::run_git_with_environment_limit(
            &self.path,
            &args(&["--no-replace-objects", "cat-file", "commit", sha]),
            None,
            None,
            environment,
            16 * 1024 * 1024,
        )
        .await?;
        anyhow::ensure!(object.code == 0, "Cannot read the created commit");
        let index_tree = crate::process::run_git_with_environment(
            &self.path,
            &args(&["write-tree"]),
            None,
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(index_tree.code == 0, "Cannot verify the hook-visible index");
        Ok(json!({
            "replacement_or_graft": !replacements.stdout.is_empty() || grafts,
            "object_base64": STANDARD.encode(&object.stdout),
            "hook_index_tree": output_line(&index_tree.stdout, "hook index tree")?,
        }))
    }

    /// Read the private validation/commit HEAD tip without treating an unborn
    /// or missing commit as a transport failure.
    pub(crate) async fn assisted_commit_private_tip(
        &self,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        validate_environment(environment)?;
        let output = crate::process::run_git_with_environment(
            &self.path,
            &args(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                "HEAD^{commit}",
            ]),
            None,
            None,
            environment,
        )
        .await?;
        anyhow::ensure!(
            matches!(output.code, 0 | 1),
            "Cannot read the private HEAD commit"
        );
        if output.code == 1 {
            return Ok(json!({"sha":Value::Null}));
        }
        let sha = output_line(&output.stdout, "private HEAD commit")?;
        object_id(&sha, "private HEAD commit")?;
        Ok(json!({"sha":sha}))
    }

    /// Produce the byte-exact frozen object diff used to review and split a
    /// selected assisted commit. Paths are literals; diff drivers are disabled.
    pub(crate) async fn assisted_commit_frozen_diff(
        &self,
        base: &str,
        tree: &str,
        paths: &[GitPath],
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        object_id(base, "frozen diff base")?;
        object_id(tree, "frozen diff tree")?;
        for key in environment.keys() {
            anyhow::ensure!(
                assisted_diff_environment_key(key),
                "unsupported assisted diff environment key"
            );
        }
        anyhow::ensure!(
            !paths.is_empty() && paths.len() <= 1024,
            "select at least one frozen diff path"
        );
        let mut command = args(&[
            "-c",
            "diff.suppressBlankEmpty=false",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-color",
            "--patch",
            "--end-of-options",
            base,
            tree,
            "--",
        ]);
        for path in paths {
            command.push(validate_index_tree_path(path)?);
        }
        let output = crate::process::run_git_with_environment_limit(
            &self.path,
            &command,
            None,
            None,
            environment,
            64 * 1024 * 1024,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot create the frozen assisted diff");
        Ok(json!({"bytes_base64":STANDARD.encode(output.stdout)}))
    }

    /// Run native commit-message validation in a host-owned scratch Git
    /// directory. Objects remain those of the private assisted object view;
    /// only refs, index, HEAD, and configuration are scratch state.
    pub(crate) async fn assisted_commit_validate_messages(
        &self,
        initial_sha: Option<&str>,
        trees: &[String],
        messages: &[String],
        sign_off: bool,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        validate_environment(environment)?;
        anyhow::ensure!(
            !trees.is_empty() && trees.len() == messages.len() && trees.len() <= 512,
            "invalid native message validation plan"
        );
        if let Some(initial) = initial_sha {
            object_id(initial, "initial validation commit")?;
        }
        let mut total_message_bytes = 0usize;
        for tree in trees {
            object_id(tree, "validation tree")?;
        }
        for message in messages {
            anyhow::ensure!(
                message.len() <= 1024 * 1024 && !message.contains('\0'),
                "invalid validation commit message"
            );
            total_message_bytes = total_message_bytes.saturating_add(message.len());
        }
        anyhow::ensure!(
            total_message_bytes <= 16 * 1024 * 1024,
            "validation commit messages are too large"
        );
        let source_git_dir = environment
            .get("GIT_DIR")
            .and_then(|value| value.as_deref())
            .ok_or_else(|| {
                anyhow::anyhow!("native validation requires the private Git directory")
            })?;
        anyhow::ensure!(
            Path::new(source_git_dir).is_absolute() && source_git_dir.len() <= 65536,
            "invalid private Git directory"
        );
        let config = std::fs::read(Path::new(source_git_dir).join("config"))?;
        anyhow::ensure!(
            config.len() <= 1024 * 1024,
            "private configuration is too large"
        );

        let scratch = TemporaryIndexDirectory::create()?;
        std::fs::create_dir_all(scratch.path.join("objects"))?;
        std::fs::create_dir_all(scratch.path.join("refs").join("heads"))?;
        std::fs::write(scratch.path.join("config"), config)?;
        std::fs::write(
            scratch.path.join("HEAD"),
            match initial_sha {
                Some(sha) => format!("{sha}\n"),
                None => "ref: refs/heads/message-validation\n".to_owned(),
            },
        )?;
        let mut hooks_argument = std::ffi::OsString::from("core.hooksPath=");
        hooks_argument.push(scratch.path.join("no-hooks").as_os_str());
        let mut scratch_environment = environment.clone();
        let scratch_path = scratch.path.to_string_lossy().into_owned();
        let index_path = scratch.path.join("index").to_string_lossy().into_owned();
        scratch_environment.insert("GIT_DIR".to_owned(), Some(scratch_path.clone()));
        scratch_environment.insert("GIT_COMMON_DIR".to_owned(), Some(scratch_path));
        scratch_environment.insert("GIT_INDEX_FILE".to_owned(), Some(index_path));

        let mut objects = Vec::new();
        for (tree, message) in trees.iter().zip(messages) {
            let read_tree = crate::process::run_git_with_environment(
                &self.path,
                &[
                    "-c".into(),
                    hooks_argument.clone(),
                    "-c".into(),
                    "commit.gpgsign=false".into(),
                    "read-tree".into(),
                    tree.clone().into(),
                ],
                None,
                None,
                &scratch_environment,
            )
            .await?;
            anyhow::ensure!(
                read_tree.code == 0,
                "Cannot prepare native message validation"
            );
            let mut commit_command = vec![
                "-c".into(),
                hooks_argument.clone(),
                "-c".into(),
                "commit.gpgsign=false".into(),
                "commit".into(),
                "--allow-empty".into(),
                "--no-verify".into(),
                "-F".into(),
                "-".into(),
            ];
            if sign_off {
                commit_command.push("--signoff".into());
            }
            let commit = crate::process::run_git_with_environment(
                &self.path,
                &commit_command,
                Some(message.as_bytes().to_vec()),
                None,
                &scratch_environment,
            )
            .await?;
            anyhow::ensure!(commit.code == 0, "Cannot create native validation commit");
            let object = crate::process::run_git_with_environment(
                &self.path,
                &args(&["cat-file", "commit", "HEAD"]),
                None,
                None,
                &scratch_environment,
            )
            .await?;
            anyhow::ensure!(object.code == 0, "Cannot read native validation commit");
            objects.push(STANDARD.encode(object.stdout));
        }
        Ok(json!({"objects_base64":objects}))
    }

    /// Construct a tree through a private scratch index, optionally returning
    /// the resulting index bytes for the transaction's final synchronization
    /// check. The renderer never controls the scratch index location.
    pub(crate) async fn assisted_commit_index_tree(
        &self,
        base: Option<&str>,
        entries: &[crate::AssistedCommitIndexTreeEntry],
        index_entries_base64: Option<&str>,
        include_index: bool,
        use_environment_index: bool,
        environment: &BTreeMap<String, Option<String>>,
    ) -> Result<Value> {
        validate_environment(environment)?;
        if let Some(base) = base {
            object_id(base, "base tree object")?;
        }
        anyhow::ensure!(entries.len() <= 100_000, "too many assisted index entries");
        anyhow::ensure!(
            !(index_entries_base64.is_some() && !entries.is_empty()),
            "raw and typed assisted index entries cannot be mixed"
        );

        let mut input = Vec::new();
        if let Some(encoded) = index_entries_base64 {
            anyhow::ensure!(
                encoded.len() <= 512 * 1024 * 1024,
                "assisted index entries are too large"
            );
            input = STANDARD.decode(encoded)?;
            anyhow::ensure!(
                input.len() <= 384 * 1024 * 1024,
                "assisted index entries are too large"
            );
            crate::assisted_head::validate_index_entries(&input)?;
        } else {
            let zero_length = base.map_or(40, str::len);
            for entry in entries {
                let path = validate_index_tree_path(&entry.path)?;
                input.extend_from_slice(b"0 ");
                input.extend_from_slice("0".repeat(zero_length).as_bytes());
                input.push(b'\t');
                input.extend_from_slice(path.as_encoded_bytes());
                input.push(0);
            }
            for entry in entries {
                let path = validate_index_tree_path(&entry.path)?;
                match (&entry.mode, &entry.sha) {
                    (Some(mode), Some(sha)) => {
                        anyhow::ensure!(
                            matches!(mode.as_str(), "100644" | "100755" | "120000"),
                            "invalid assisted index entry mode"
                        );
                        object_id(sha, "assisted index entry object")?;
                        input.extend_from_slice(mode.as_bytes());
                        input.push(b' ');
                        input.extend_from_slice(sha.as_bytes());
                        input.push(b'\t');
                        input.extend_from_slice(path.as_encoded_bytes());
                        input.push(0);
                    }
                    (None, None) => {}
                    _ => anyhow::bail!("assisted index entry mode and object must be paired"),
                }
            }
        }
        anyhow::ensure!(
            input.len() <= 384 * 1024 * 1024,
            "assisted index input is too large"
        );

        let scratch = TemporaryIndexDirectory::create()?;
        let index = scratch.path.join("index");
        let mut hooks_path = std::ffi::OsString::from("core.hooksPath=");
        hooks_path.push(scratch.path.join("no-hooks"));
        let mut environment = environment.clone();
        let environment_index = if use_environment_index {
            let Some(Some(path)) = environment.get("GIT_INDEX_FILE") else {
                anyhow::bail!("assisted environment index is unavailable");
            };
            anyhow::ensure!(
                !path.is_empty() && path.len() <= 4096 && Path::new(path).is_absolute(),
                "invalid assisted environment index"
            );
            true
        } else {
            environment.remove("GIT_INDEX_FILE");
            false
        };
        let index_override = (!environment_index).then_some(index.as_path());
        let read_args = match base {
            Some(base) => args(&["read-tree", base]),
            None => args(&["read-tree", "--empty"]),
        };
        let with_no_hooks = |command: &[std::ffi::OsString]| {
            let mut result = vec![std::ffi::OsString::from("-c"), hooks_path.clone()];
            result.extend_from_slice(command);
            result
        };
        let output = crate::process::run_git_with_environment(
            &self.path,
            &with_no_hooks(&read_args),
            None,
            index_override,
            &environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot initialize assisted scratch index");
        if !input.is_empty() {
            let output = crate::process::run_git_with_environment(
                &self.path,
                &with_no_hooks(&args(&["update-index", "-z", "--index-info"])),
                Some(input),
                index_override,
                &environment,
            )
            .await?;
            anyhow::ensure!(output.code == 0, "Cannot update assisted scratch index");
        }
        let output = crate::process::run_git_with_environment(
            &self.path,
            &with_no_hooks(&args(&["write-tree"])),
            None,
            index_override,
            &environment,
        )
        .await?;
        anyhow::ensure!(output.code == 0, "Cannot write assisted tree");
        let tree = output_line(&output.stdout, "written tree object")?;
        object_id(&tree, "written tree object")?;
        let index_base64 = if include_index {
            anyhow::ensure!(
                !use_environment_index,
                "environment index bytes are not exported"
            );
            let bytes = std::fs::read(&index)?;
            anyhow::ensure!(
                bytes.len() <= 384 * 1024 * 1024,
                "assisted index is too large"
            );
            Value::String(STANDARD.encode(bytes))
        } else {
            Value::Null
        };
        Ok(json!({"tree":tree,"index_base64":index_base64}))
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
