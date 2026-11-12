//! Discover the repository and object stores that a Git mutation can affect.
use crate::{args, cancellation, os_from_bytes, process};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct ProtectedResources {
    paths: Vec<String>,
    object_directories: Vec<String>,
    object_routing_diagnostics: String,
}

fn utf8_path(path: &Path) -> Result<String> {
    anyhow::ensure!(path.is_absolute(), "Git returned a relative resource path");
    Ok(path
        .to_str()
        .context("Git resource path is not UTF-8")?
        .to_owned())
}

async fn git_output(path: &Path, command: &[&str]) -> Result<process::RawOutput> {
    let output = process::run(path, &args(command)).await?;
    if output.code != 0 {
        bail!(
            "Git resource discovery failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn output_path(mut output: Vec<u8>) -> Result<PathBuf> {
    anyhow::ensure!(
        output.pop() == Some(b'\n'),
        "Git resource path is unterminated"
    );
    if output.last() == Some(&b'\r') {
        output.pop();
    }
    anyhow::ensure!(
        !output.is_empty() && !output.contains(&0),
        "invalid Git resource path"
    );
    let path = PathBuf::from(os_from_bytes(&output)?);
    utf8_path(&path)?;
    Ok(path)
}

/// Decode Git's C-style quoting for `count-objects` alternate paths.
fn alternate_path(value: &[u8]) -> Result<PathBuf> {
    let decoded = if value.first() != Some(&b'"') {
        value.to_vec()
    } else {
        anyhow::ensure!(
            value.len() >= 2 && value.last() == Some(&b'"'),
            "unterminated alternate path"
        );
        let mut result = Vec::new();
        let mut i = 1;
        while i < value.len() - 1 {
            let b = value[i];
            if b != b'\\' {
                anyhow::ensure!(b != b'"', "invalid alternate path quote");
                result.push(b);
                i += 1;
                continue;
            }
            i += 1;
            anyhow::ensure!(i < value.len() - 1, "invalid alternate path escape");
            let escaped = value[i];
            let byte = match escaped {
                b'a' => 7,
                b'b' => 8,
                b'f' => 12,
                b'n' => 10,
                b'r' => 13,
                b't' => 9,
                b'v' => 11,
                b'\\' | b'"' => escaped,
                b'0'..=b'7' => {
                    anyhow::ensure!(
                        i + 2 < value.len() - 1,
                        "invalid alternate path octal escape"
                    );
                    let digits = &value[i..i + 3];
                    anyhow::ensure!(
                        digits.iter().all(|b| (b'0'..=b'7').contains(b)),
                        "invalid alternate path octal escape"
                    );
                    let n = u16::from(digits[0] - b'0') * 64
                        + u16::from(digits[1] - b'0') * 8
                        + u16::from(digits[2] - b'0');
                    anyhow::ensure!(n <= 255, "invalid alternate path octal escape");
                    i += 2;
                    n as u8
                }
                _ => bail!("unsupported alternate path escape"),
            };
            result.push(byte);
            i += 1;
        }
        result
    };
    anyhow::ensure!(
        !decoded.is_empty() && !decoded.contains(&0),
        "invalid alternate path"
    );
    let path = PathBuf::from(os_from_bytes(&decoded)?);
    utf8_path(&path)?;
    Ok(path)
}

/// Preserve both caller-visible aliases and canonical paths, including the
/// complete transitive alternate object graph expanded by Git itself.
pub async fn protected_resources(path: &Path) -> Result<ProtectedResources> {
    utf8_path(path)?;
    let directory = output_path(
        git_output(path, &["rev-parse", "--absolute-git-dir"])
            .await?
            .stdout,
    )?;
    let common = output_path(
        git_output(
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?
        .stdout,
    )?;
    let object = output_path(
        git_output(
            path,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
        )
        .await?
        .stdout,
    )?;
    let alternates =
        git_output(path, &["-c", "core.quotePath=false", "count-objects", "-v"]).await?;
    let mut objects = vec![object];
    for line in alternates.stdout.split(|b| *b == b'\n') {
        if let Some(value) = line.strip_prefix(b"alternate: ") {
            objects.push(alternate_path(value.strip_suffix(b"\r").unwrap_or(value))?);
        }
    }
    let diagnostics = String::from_utf8(alternates.stderr)
        .context("Git routing diagnostics are not UTF-8")?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with("warning: garbage found: "))
        .collect::<Vec<_>>()
        .join("\n");
    let original = [vec![path.to_path_buf(), directory, common], objects.clone()].concat();
    let canonical = cancellation::blocking({
        let original = original.clone();
        move || {
            original
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .map(crate::repository_discovery::display_canonical)
                        .with_context(|| format!("cannot resolve resource path {}", p.display()))
                })
                .collect::<Result<Vec<_>>>()
        }
    })
    .await?;
    let paths = original
        .iter()
        .chain(&canonical)
        .map(|p| utf8_path(p))
        .collect::<Result<Vec<_>>>()?;
    let object_directories = objects
        .iter()
        .map(|p| utf8_path(p))
        .collect::<Result<Vec<_>>>()?;
    Ok(ProtectedResources {
        paths,
        object_directories,
        object_routing_diagnostics: diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ambiguous_git_path_quoting() {
        assert_eq!(
            alternate_path(br#""/tmp/tab\tquote\"""#).unwrap(),
            PathBuf::from("/tmp/tab\tquote\"")
        );
        for value in [
            br#""/tmp/no-end"#.as_slice(),
            br#""/tmp/\999""#,
            br#""/tmp/\400""#,
            br#""/tmp/\q""#,
        ] {
            assert!(alternate_path(value).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protects_linked_worktrees_and_transitive_quoted_alternates() {
        use std::{fs, os::unix::fs::symlink, process::Command};
        let root = std::env::temp_dir().join(format!("desktop-resources-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source\t\"unicode-å");
        let first = root.join("first");
        let second = root.join("second");
        let linked = root.join("linked");
        let run = |cwd: &Path, arguments: &[&str]| {
            let output = Command::new("git")
                .current_dir(cwd)
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&root, &["init", "-q", source.to_str().unwrap()]);
        fs::write(source.join("file"), "content\n").unwrap();
        run(&source, &["add", "file"]);
        run(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "root",
            ],
        );
        run(
            &root,
            &[
                "clone",
                "-q",
                "--shared",
                source.to_str().unwrap(),
                first.to_str().unwrap(),
            ],
        );
        run(
            &root,
            &[
                "clone",
                "-q",
                "--shared",
                first.to_str().unwrap(),
                second.to_str().unwrap(),
            ],
        );
        run(
            &second,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let alias = root.join("linked-alias");
        symlink(&linked, &alias).unwrap();
        let resources = protected_resources(&alias).await.unwrap();
        assert_eq!(resources.paths[0], alias.to_str().unwrap());
        assert!(
            resources
                .paths
                .contains(&linked.canonicalize().unwrap().to_str().unwrap().to_owned())
        );
        assert!(
            resources.paths.contains(
                &source
                    .join(".git/objects")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            )
        );
        assert!(
            resources.paths.contains(
                &second
                    .join(".git")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            )
        );
        assert!(
            resources
                .object_directories
                .iter()
                .any(|p| p.contains("source\t\"unicode-å"))
        );
        assert!(resources.object_routing_diagnostics.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
