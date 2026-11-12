//! Repository-root ignore file access for the browser and native hosts.
use crate::{Repository, cancellation, creation, settings};
use anyhow::{Result, ensure};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

const MAX_BYTES: u64 = 1024 * 1024;
type IgnoreGate = tokio::sync::Mutex<()>;
static IGNORE_GATES: OnceLock<Mutex<HashMap<PathBuf, Weak<IgnoreGate>>>> = OnceLock::new();

fn ignore_gate(path: &Path) -> Arc<IgnoreGate> {
    let mut gates = IGNORE_GATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(path).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(IgnoreGate::new(()));
    gates.insert(path.to_owned(), Arc::downgrade(&gate));
    gate
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitIgnoreAction {
    Read {},
    Save { text: String },
    Append { text: String },
}

impl Repository {
    pub(crate) async fn gitignore(&self, action: GitIgnoreAction) -> Result<Value> {
        // Wry opens a separate Repository handle per request. Serialize this
        // read-modify-write workflow across handles to avoid losing app edits.
        let gate = ignore_gate(&self.path);
        let _guard = cancellation::wait(gate.lock()).await?;
        if matches!(action, GitIgnoreAction::Read {}) {
            let root = self.path.clone();
            return Ok(json!(cancellation::blocking(move || read(&root)).await?));
        }
        let auto = settings::git_read_setting(
            &self.path,
            settings::GitReadSettingKey::AutoCrlf,
            false,
            false,
        )
        .await?;
        let safe = settings::git_read_setting(
            &self.path,
            settings::GitReadSettingKey::SafeCrlf,
            false,
            false,
        )
        .await?;
        let root = self.path.clone();
        cancellation::blocking(move || {
            match action {
                GitIgnoreAction::Read {} => unreachable!(),
                GitIgnoreAction::Save { text } => save(&root, &text, auto.as_str(), safe.as_str())?,
                GitIgnoreAction::Append { text } => {
                    let current = read(&root)?.unwrap_or_default();
                    let formatted = format_ignore(&current, auto.as_str(), safe.as_str());
                    let combined =
                        format_ignore(&(formatted + &text), auto.as_str(), safe.as_str());
                    save(&root, &combined, auto.as_str(), safe.as_str())?;
                }
            }
            Ok(Value::Null)
        })
        .await
    }
}

fn read(root: &Path) -> Result<Option<String>> {
    let dir = Dir::open_ambient_dir(root, ambient_authority())?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match dir.open_with(".gitignore", &options) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is not an absent file.
            if dir.symlink_metadata(".gitignore").is_ok() {
                anyhow::bail!("Cannot use a symbolic link as the root .gitignore file");
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Root .gitignore is not a regular file"
    );
    let mut data = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= MAX_BYTES,
        "Root .gitignore exceeds 1 MiB"
    );
    Ok(Some(String::from_utf8_lossy(&data).into_owned()))
}

fn save(root: &Path, text: &str, auto: Option<&str>, safe: Option<&str>) -> Result<()> {
    ensure!(
        text.len() as u64 <= MAX_BYTES,
        "Root .gitignore exceeds 1 MiB"
    );
    if text.is_empty() {
        let dir = Dir::open_ambient_dir(root, ambient_authority())?;
        let metadata = dir.symlink_metadata(".gitignore")?;
        ensure!(
            metadata.is_file(),
            "Cannot remove a non-regular root .gitignore file"
        );
        cancellation::Cancellation::current().check()?;
        dir.remove_file(".gitignore")?;
    } else {
        let formatted = format_ignore(text, auto, safe);
        ensure!(
            formatted.len() as u64 <= MAX_BYTES,
            "Root .gitignore exceeds 1 MiB"
        );
        creation::write_file(
            root,
            creation::CreationFile::Gitignore,
            formatted.as_bytes(),
        )?;
    }
    Ok(())
}

fn format_ignore(text: &str, auto: Option<&str>, safe: Option<&str>) -> String {
    if auto == Some("true") && safe == Some("true") {
        let mut output = String::with_capacity(text.len() + 2);
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' || ch == '\n' {
                if chars
                    .peek()
                    .is_some_and(|next| (*next == '\r' || *next == '\n') && *next != ch)
                {
                    chars.next();
                }
                output.push_str("\r\n");
            } else {
                output.push(ch);
            }
        }
        output.push_str("\r\n");
        return output;
    }
    if text.is_empty() || text.ends_with('\n') {
        return text.to_owned();
    }
    format!(
        "{text}{}",
        if auto.is_none() || auto == Some("true") {
            "\n"
        } else {
            "\r\n"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Operation;

    #[tokio::test]
    async fn reads_saves_appends_and_deletes_in_a_real_repository() {
        let root = std::env::temp_dir().join(format!("desktop-ignore-{}", uuid::Uuid::new_v4()));
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        for (key, value) in [("core.autocrlf", "true"), ("core.safecrlf", "false")] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["config", "--local", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let repo = Repository::open(&root).await.unwrap();
        let run = |action| repo.execute(Operation::GitIgnore { action });
        assert_eq!(run(GitIgnoreAction::Read {}).await.unwrap(), Value::Null);
        run(GitIgnoreAction::Save { text: "one".into() })
            .await
            .unwrap();
        assert_eq!(run(GitIgnoreAction::Read {}).await.unwrap(), json!("one\n"));
        run(GitIgnoreAction::Append {
            text: "two\nthree".into(),
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "one\ntwo\nthree\n"
        );
        run(GitIgnoreAction::Save {
            text: String::new(),
        })
        .await
        .unwrap();
        assert_eq!(run(GitIgnoreAction::Read {}).await.unwrap(), Value::Null);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_existing_line_ending_rules() {
        assert_eq!(
            format_ignore("a\r\n\rb", Some("true"), Some("true")),
            "a\r\n\r\nb\r\n"
        );
        assert_eq!(format_ignore("a", Some("false"), None), "a\r\n");
        assert_eq!(format_ignore("a\n", None, None), "a\n");
    }

    #[tokio::test]
    async fn concurrent_handles_do_not_lose_appends() {
        let root =
            std::env::temp_dir().join(format!("desktop-ignore-gate-{}", uuid::Uuid::new_v4()));
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        for (key, value) in [("core.autocrlf", "true"), ("core.safecrlf", "false")] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["config", "--local", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let first = Repository::open(&root).await.unwrap();
        let second = Repository::open(&root).await.unwrap();
        let (a, b) = tokio::join!(
            first.execute(Operation::GitIgnore {
                action: GitIgnoreAction::Append {
                    text: "first".into()
                }
            }),
            second.execute(Operation::GitIgnore {
                action: GitIgnoreAction::Append {
                    text: "second".into()
                }
            })
        );
        a.unwrap();
        b.unwrap();
        let text = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(text == "first\nsecond\n" || text == "second\nfirst\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_preserves_target() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("desktop-ignore-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("target");
        std::fs::write(&target, "untouched").unwrap();
        symlink(&target, root.join(".gitignore")).unwrap();
        assert!(read(&root).is_err());
        assert!(save(&root, "changed", None, None).is_err());
        assert!(save(&root, "", None, None).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
        std::fs::remove_dir_all(root).unwrap();
    }
}
