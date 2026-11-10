//! Port of `app/src/lib/git/core.ts`, `spawn.ts` and `environment.ts`.
//!
//! GitHub Desktop does not link libgit2 — it spawns the bundled Git binary via
//! dugite. We keep that design: one `GitEngine` owns the executable path and
//! the environment (credential trampoline, hooks, no terminal prompts), and
//! every git module above it builds argument vectors against `run`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{AppError, GitError, GitErrorKind, Result, parse_git_error};

/// Dugite caps captured git output at 256 KB; keep the same bound so a runaway
/// command cannot balloon the IPC payload.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl GitOutput {
    pub fn stdout_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GitOptions {
    /// Exit codes that should not throw. Electron defaults to `Set([0])`.
    pub success_exit_codes: Vec<i32>,
    /// Classified errors treated as expected results rather than exceptions.
    pub expected_errors: Vec<GitErrorKind>,
    pub env: HashMap<String, String>,
    pub stdin: Option<String>,
}

impl GitOptions {
    pub fn new() -> Self {
        Self {
            success_exit_codes: vec![0],
            ..Self::default()
        }
    }
    pub fn expect(mut self, kind: GitErrorKind) -> Self {
        self.expected_errors.push(kind);
        self
    }
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
    pub fn success(mut self, code: i32) -> Self {
        if !self.success_exit_codes.contains(&code) {
            self.success_exit_codes.push(code);
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct GitEngine {
    pub binary: PathBuf,
    /// `withTrampolineEnv` / `withHooksEnv` replacements.
    pub base_env: HashMap<String, String>,
}

impl Default for GitEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl GitEngine {
    pub fn new() -> Self {
        let binary = std::env::var("DESKTOP_GIT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("git"));

        // Keep git output machine-parseable and prevent interactive credential
        // prompts from hanging the UI thread. Desktop achieves this with the
        // trampoline (GIT_ASKPASS / credential helper); the same env contract
        // is honoured here so git credentials still flow through the OS helper.
        let mut base_env = HashMap::new();
        base_env.insert("LC_ALL".into(), "C".into());
        base_env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        base_env.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
        base_env.insert("GCM_INTERACTIVE".into(), "Never".into());
        base_env.insert("GIT_PAGER".into(), "cat".into());
        base_env.insert("GIT_CONFIG_COUNT".into(), "0".into());

        Self { binary, base_env }
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run(&["--version"], Path::new("."), GitOptions::new())?;
        Ok(out.stdout.trim().to_string())
    }

    /// The single entry point for all git invocations.
    pub fn run(&self, args: &[&str], path: &Path, options: GitOptions) -> Result<GitOutput> {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .current_dir(if path.as_os_str().is_empty() {
                Path::new(".")
            } else {
                path
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.base_env)
            .envs(&options.env);

        if options.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        // Avoid a flashing console window on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(|e| {
            AppError::msg(format!(
                "Unable to start git at {:?}: {e}. Set DESKTOP_GIT_PATH to a git executable.",
                self.binary
            ))
        })?;

        if let Some(input) = &options.stdin {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(input.as_bytes());
            }
        }

        let output = child.wait_with_output().map_err(AppError::Io)?;
        let stdout = truncate(&String::from_utf8_lossy(&output.stdout));
        let stderr = truncate(&String::from_utf8_lossy(&output.stderr));
        let exit_code = output.status.code().unwrap_or(-1);

        let success_codes = if options.success_exit_codes.is_empty() {
            vec![0]
        } else {
            options.success_exit_codes.clone()
        };

        if !success_codes.contains(&exit_code) {
            let kind = parse_git_error(&stdout, &stderr);
            if !options.expected_errors.contains(&kind) && kind != GitErrorKind::None {
                return Err(AppError::Git(GitError::new(
                    kind,
                    args.iter().map(|s| s.to_string()).collect(),
                    stdout,
                    stderr,
                    Some(exit_code),
                )));
            }
        }

        Ok(GitOutput {
            stdout,
            stderr,
            exit_code,
        })
    }

    /// Convenience for commands whose stdout is the result and whose failure
    /// should propagate as an `AppError::Git`.
    pub fn output(&self, args: &[&str], path: &Path) -> Result<String> {
        Ok(self.run(args, path, GitOptions::new())?.stdout)
    }

    /// `git config` reader, replacing `getConfigValue`.
    pub fn config(&self, key: &str, path: &Path) -> Result<Option<String>> {
        let opts = GitOptions::new().success(1);
        let out = self.run(&["config", "--get", key], path, opts)?;
        if out.exit_code != 0 {
            return Ok(None);
        }
        Ok(Some(out.stdout.trim().to_string()))
    }

    pub fn set_config(&self, key: &str, value: &str, path: &Path) -> Result<()> {
        self.run(&["config", key, value], path, GitOptions::new())?;
        Ok(())
    }

    pub fn is_repository(&self, path: &Path) -> bool {
        self.run(
            &["rev-parse", "--is-inside-work-tree"],
            path,
            GitOptions::new().success(128),
        )
        .map(|o| o.exit_code == 0 && o.stdout.trim() == "true")
        .unwrap_or(false)
    }
}

fn truncate(value: &str) -> String {
    if value.len() <= MAX_OUTPUT_BYTES {
        return value.to_string();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = value[..end].to_string();
    out.push_str("\n[output truncated at 256 KB]");
    out
}

/// Split an argument string the way `git()` callers expect when building
/// commands from user-supplied refspecs. Never invokes a shell.
pub fn args_from_str(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}
