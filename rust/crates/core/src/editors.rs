//! User-selected editor commands, resolved without a shell and bound to a repository.
use crate::{GitPath, Repository, cancellation, discard_fs};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalEditor {
    #[default]
    Automatic,
    /// An editor selected from the host's fixed discovery list.
    Named { name: String },
    Command {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        terminal: bool,
    },
}

/// A custom integration selected through Desktop's editor preferences.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCustomEditor {
    pub program: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub macos_app: bool,
}

impl DesktopCustomEditor {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.program.is_empty() && self.program.len() <= 4096 && !self.program.contains('\0'),
            "custom editor program must be a nonempty path"
        );
        anyhow::ensure!(
            self.args.len() <= 64
                && self.args.iter().map(String::len).sum::<usize>() <= 65536
                && self.args.iter().all(|arg| !arg.contains('\0')),
            "custom editor arguments exceed limits or contain NUL"
        );
        Ok(())
    }
}
impl ExternalEditor {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Self::Named { name } = self {
            anyhow::ensure!(
                !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
                "invalid editor name"
            );
        }
        if let Self::Command { program, args, .. } = self {
            anyhow::ensure!(
                !program.is_empty() && !program.contains('\0') && program.len() <= 4096,
                "editor program must be a nonempty executable name or path"
            );
            anyhow::ensure!(
                args.len() <= 64
                    && args.iter().map(String::len).sum::<usize>() <= 65536
                    && args.iter().all(|arg| !arg.contains('\0')),
                "editor arguments exceed limits or contain NUL"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
struct AvailableEditor {
    name: &'static str,
    program: PathBuf,
    terminal: bool,
    directories: bool,
}
fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn absolute_executable(path: &Path) -> Option<PathBuf> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Keep the final symlink name: multicall editors use argv[0] to choose
    // their mode, while an absolute parent prevents CWD from changing lookup.
    Some(parent.canonicalize().ok()?.join(path.file_name()?))
}
fn locate(program: &OsStr) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 || program_path.is_absolute() {
        return executable(program_path)
            .then(|| absolute_executable(program_path))
            .flatten();
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    for parent in std::env::split_paths(&search) {
        let candidate = parent.join(program);
        if executable(&candidate) {
            return absolute_executable(&candidate);
        }
        #[cfg(windows)]
        for extension in ["exe", "com", "cmd", "bat"] {
            let candidate = candidate.with_extension(extension);
            if executable(&candidate) {
                return absolute_executable(&candidate);
            }
        }
    }
    None
}

/// Validate an editor/terminal selected in Desktop's integration preferences.
pub async fn validate_custom_integration_path(path: String) -> Result<Value> {
    anyhow::ensure!(
        path.len() <= 4096 && !path.contains('\0'),
        "invalid integration path"
    );
    if path.is_empty() {
        return Ok(json!({"is_valid": false}));
    }
    let candidate = PathBuf::from(&path);
    let is_file = cancellation::blocking({
        let candidate = candidate.clone();
        move || {
            if !candidate
                .metadata()
                .is_ok_and(|metadata| metadata.is_file())
            {
                return Ok(false);
            }
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(candidate.as_os_str().as_bytes())?;
                // Match the real-user execute-access check used by Node's fs.access.
                Ok(unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
            }
            #[cfg(windows)]
            {
                Ok(windows_direct_executable(&candidate))
            }
            #[cfg(not(any(unix, windows)))]
            Ok(false)
        }
    })
    .await?;
    if is_file {
        return Ok(json!({"is_valid": true}));
    }
    #[cfg(target_os = "macos")]
    {
        let is_app = candidate.extension() == Some(OsStr::new("app"))
            && candidate
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir());
        if is_app {
            let mut command = tokio::process::Command::new("/usr/bin/mdls");
            command
                .args(["-name", "kMDItemCFBundleIdentifier", "-raw"])
                .arg(&candidate)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            if let Ok(Ok(output)) =
                tokio::time::timeout(std::time::Duration::from_secs(3), command.output()).await
            {
                if output.status.success() && output.stdout.len() <= 4096 {
                    if let Ok(text) = String::from_utf8(output.stdout) {
                        let bundle = text.trim();
                        if !bundle.is_empty()
                            && bundle != "(null)"
                            && !bundle.chars().any(char::is_control)
                        {
                            return Ok(json!({"is_valid": true, "bundle_id": bundle}));
                        }
                    }
                }
            }
        }
    }
    Ok(json!({"is_valid": false}))
}

/// Resolve a macOS application bundle identifier through Launch Services.
///
/// AppleScript's `path to application id` is the stable host interface used
/// here; the bundle identifier is restricted before interpolation into the
/// script so it cannot alter the fixed expression.
pub async fn resolve_macos_app_path(bundle_id: String) -> Result<String> {
    anyhow::ensure!(
        bundle_id.len() <= 256
            && !bundle_id.is_empty()
            && bundle_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        "invalid macOS bundle identifier"
    );
    #[cfg(target_os = "macos")]
    {
        let script = format!("POSIX path of (path to application id \"{bundle_id}\")");
        let mut command = tokio::process::Command::new("/usr/bin/osascript");
        command
            .args(["-e", &script])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(3), command.output()).await??;
        anyhow::ensure!(output.status.success(), "could not find the application");
        anyhow::ensure!(output.stdout.len() <= 4096, "invalid application path");
        let path = String::from_utf8(output.stdout)?;
        let path = path.trim_end_matches(['\r', '\n']).trim_end_matches('/');
        anyhow::ensure!(
            path.starts_with('/') && path.ends_with(".app") && !path.contains('\0'),
            "invalid application path"
        );
        return Ok(path.to_owned());
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("macOS application resolution is only available on macOS")
}

/// Parse a Windows command line using the API used by the legacy native addon.
pub async fn parse_windows_integration_arguments(arguments: String) -> Result<Vec<String>> {
    anyhow::ensure!(
        arguments.len() <= 65536 && !arguments.contains('\0'),
        "invalid integration arguments"
    );
    #[cfg(windows)]
    {
        return cancellation::blocking(move || {
            use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
            if arguments.is_empty() {
                return Ok(Vec::new());
            }
            let wide: Vec<u16> = arguments.encode_utf16().chain(std::iter::once(0)).collect();
            let mut count = 0;
            let pointer = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut count) };
            anyhow::ensure!(
                !pointer.is_null(),
                "could not parse Windows integration arguments"
            );
            struct Arguments(*mut *mut u16);
            impl Drop for Arguments {
                fn drop(&mut self) {
                    unsafe {
                        LocalFree(self.0.cast());
                    }
                }
            }
            let guard = Arguments(pointer);
            anyhow::ensure!((0..=64).contains(&count), "too many integration arguments");
            let mut result = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let value = unsafe { *guard.0.add(index) };
                anyhow::ensure!(!value.is_null(), "invalid Windows integration argument");
                let mut length = 0;
                while unsafe { *value.add(length) } != 0 {
                    length += 1;
                }
                let chars = unsafe { std::slice::from_raw_parts(value, length) };
                result.push(String::from_utf16(chars)?);
            }
            Ok(result)
        })
        .await;
    }
    #[cfg(not(windows))]
    anyhow::bail!("Windows integration arguments are only available on Windows")
}
/// Custom integrations resolve the exact name and never try shell wrappers.
fn locate_custom_in(program: &OsStr, search: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 || path.is_absolute() {
        return executable(path)
            .then(|| absolute_executable(path))
            .flatten();
    }
    std::env::split_paths(search)
        .map(|directory| directory.join(program))
        .find_map(|candidate| {
            executable(&candidate)
                .then(|| absolute_executable(&candidate))
                .flatten()
        })
}

#[cfg(any(windows, test))]
fn windows_direct_executable(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("com"))
}

fn discover() -> Vec<AvailableEditor> {
    let mut found = Vec::new();
    for (name, command, terminal, directories) in [
        ("Visual Studio Code", "code", false, true),
        ("Visual Studio Code Insiders", "code-insiders", false, true),
        ("VSCodium", "codium", false, true),
        ("Sublime Text", "subl", false, true),
        ("Zed", "zed", false, true),
        ("Kate", "kate", false, true),
        ("Gedit", "gedit", false, false),
        ("Mousepad", "mousepad", false, false),
        ("Notepad++", "notepad++", false, false),
        ("Notepad", "notepad.exe", false, false),
        ("Neovim", "nvim", true, true),
        ("Vim", "vim", true, true),
        ("Nano", "nano", true, false),
        ("Vi", "vi", true, true),
        ("Micro", "micro", true, false),
    ] {
        if let Some(program) = locate(OsStr::new(command)) {
            found.push(AvailableEditor {
                name,
                program,
                terminal,
                directories,
            });
        }
    }
    #[cfg(target_os = "macos")]
    if executable(Path::new("/usr/bin/open")) {
        found.push(AvailableEditor {
            name: "TextEdit",
            program: "/usr/bin/open".into(),
            terminal: false,
            directories: false,
        });
    }
    found.sort_by_key(|editor| editor.terminal);
    found
}
/// Describe available editor executables without launching any application.
pub async fn available_editors(configuration: ExternalEditor) -> Result<Value> {
    configuration.validate()?;
    cancellation::blocking(move || Ok(json!({"configuration":configuration,"editors":discover()})))
        .await
}
/// A fixed executable and argument vector, usable with the CLI's inherited terminal.
pub struct EditorLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub directory: PathBuf,
    pub terminal: bool,
}

/// Open only the global Git configuration selected by the host's Git settings.
pub async fn open_global_git_config_editor(name: String) -> Result<Value> {
    ExternalEditor::Named { name: name.clone() }.validate()?;
    let editor = cancellation::blocking(move || {
        let editor = discover()
            .into_iter()
            .find(|editor| editor.name == name)
            .context("Selected editor was not found")?;
        anyhow::ensure!(
            !editor.terminal,
            "This editor needs an interactive terminal"
        );
        Ok(editor)
    })
    .await?;
    let path = crate::git_global_config_path().await?;
    let plan = cancellation::blocking(move || {
        let target = PathBuf::from(path)
            .canonicalize()
            .context("global Git configuration does not exist")?;
        anyhow::ensure!(target.is_file(), "global Git configuration must be a file");
        let mut args = Vec::new();
        #[cfg(target_os = "macos")]
        if editor.name == "TextEdit" {
            args.extend([OsString::from("-a"), OsString::from("TextEdit")]);
        }
        if editor.name != "Notepad" && editor.name != "Notepad++" && editor.name != "TextEdit" {
            args.push(OsString::from("--"));
        }
        args.push(target.as_os_str().to_owned());
        Ok(EditorLaunch {
            program: editor.program,
            args,
            directory: target
                .parent()
                .context("global Git configuration has no parent")?
                .to_owned(),
            terminal: editor.terminal,
        })
    })
    .await?;
    launch_editor(plan).await
}

/// Open the host's Git configuration with an explicitly configured editor.
pub async fn open_global_git_config_custom_editor(editor: DesktopCustomEditor) -> Result<Value> {
    let (editor, invocation) = cancellation::blocking(move || {
        let invocation = desktop_editor_invocation(&editor)?;
        Ok((editor, invocation))
    })
    .await?;
    let path = crate::git_global_config_path().await?;
    let plan = cancellation::blocking(move || {
        let target = PathBuf::from(path)
            .canonicalize()
            .context("global Git configuration does not exist")?;
        anyhow::ensure!(target.is_file(), "global Git configuration must be a file");
        let directory = target
            .parent()
            .context("global Git configuration has no parent")?;
        Ok(desktop_editor_plan(editor, invocation, &target, directory))
    })
    .await?;
    launch_editor(plan).await
}
fn substitute(template: &str, target: &Path, repository: &Path) -> OsString {
    let mut output = OsString::new();
    let mut remaining = template;
    while let Some(position) = remaining.find(['{']) {
        output.push(&remaining[..position]);
        remaining = &remaining[position..];
        if let Some(suffix) = remaining.strip_prefix("{path}") {
            output.push(target.as_os_str());
            remaining = suffix;
        } else if let Some(suffix) = remaining.strip_prefix("{repo}") {
            output.push(repository.as_os_str());
            remaining = suffix;
        } else {
            output.push("{");
            remaining = &remaining[1..];
        }
    }
    output.push(remaining);
    output
}

fn editor_target(repository: &Path, path: Option<GitPath>) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(repository.to_owned());
    };
    let path = discard_fs::checked_path(path)?;
    let canonical = repository
        .join(path)
        .canonicalize()
        .context("editor target does not exist")?;
    anyhow::ensure!(
        canonical.starts_with(repository),
        "editor target escapes the repository through a symlink"
    );
    anyhow::ensure!(
        canonical.is_file(),
        "editor target must be an existing file"
    );
    discard_fs::checked_path(GitPath::Bytes {
        bytes: canonical
            .strip_prefix(repository)?
            .as_os_str()
            .as_encoded_bytes()
            .to_vec(),
    })?;
    Ok(canonical)
}

fn substitute_desktop_target(template: &str, target: &Path) -> OsString {
    if matches!(template, "'%TARGET_PATH%'" | "\"%TARGET_PATH%\"") {
        return target.as_os_str().to_owned();
    }
    let mut result = OsString::new();
    let mut segments = template.split("%TARGET_PATH%").peekable();
    while let Some(segment) = segments.next() {
        result.push(segment);
        if segments.peek().is_some() {
            result.push(target.as_os_str());
        }
    }
    result
}

#[cfg(any(target_os = "macos", test))]
fn resolved_macos_application(path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        path.extension() == Some(OsStr::new("app"))
            && path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir()),
        "configured editor application is not available"
    );
    path.canonicalize()
        .context("could not resolve editor application")
}

fn desktop_editor_invocation(editor: &DesktopCustomEditor) -> Result<(PathBuf, Vec<OsString>)> {
    editor.validate()?;
    let invocation = if editor.macos_app {
        #[cfg(target_os = "macos")]
        {
            let app = resolved_macos_application(Path::new(&editor.program))?;
            (
                PathBuf::from("/usr/bin/open"),
                vec![OsString::from("-a"), app.into_os_string()],
            )
        }
        #[cfg(not(target_os = "macos"))]
        anyhow::bail!("macOS applications can only be launched on macOS")
    } else {
        #[cfg(windows)]
        anyhow::ensure!(
            windows_direct_executable(Path::new(&editor.program)),
            "custom editor must be an executable, not a shell script"
        );
        let program = locate_custom_in(
            OsStr::new(&editor.program),
            &std::env::var_os("PATH").unwrap_or_default(),
        )
        .context("Configured editor executable was not found or is not executable")?;
        #[cfg(windows)]
        anyhow::ensure!(
            windows_direct_executable(&program),
            "resolved custom editor must be an executable, not a shell script"
        );
        (program, Vec::new())
    };
    Ok(invocation)
}

fn desktop_editor_plan(
    editor: DesktopCustomEditor,
    (program, mut args): (PathBuf, Vec<OsString>),
    target: &Path,
    directory: &Path,
) -> EditorLaunch {
    args.extend(
        editor
            .args
            .iter()
            .map(|arg| substitute_desktop_target(arg, target)),
    );
    EditorLaunch {
        program,
        args,
        directory: directory.to_owned(),
        terminal: false,
    }
}
impl Repository {
    /// Resolve a repository-relative file, rejecting metadata and symlink traversal.
    pub async fn editor_launch(
        &self,
        path: Option<GitPath>,
        configuration: ExternalEditor,
    ) -> Result<EditorLaunch> {
        configuration.validate()?;
        let repository = self.path().to_owned();
        cancellation::blocking(move || {
            let target = editor_target(&repository, path)?;
            let (program, templates, terminal) = match configuration {
                ExternalEditor::Automatic | ExternalEditor::Named { .. } => {
                    let name = match &configuration {
                        ExternalEditor::Named { name } => Some(name.as_str()),
                        _ => None,
                    };
                    let editor = discover().into_iter().find(|editor| (target.is_file() || editor.directories) && name.is_none_or(|name| editor.name == name))
                        .context("No suitable editor found. Configure an editor in Settings or use the CLI preferences command.")?;
                    let mut templates = Vec::<String>::new();
                    #[cfg(target_os = "macos")]
                    if editor.name == "TextEdit" {templates.extend(["-a".into(),"TextEdit".into()]);}
                    if editor.name != "Notepad" && editor.name != "Notepad++" && editor.name != "TextEdit" {templates.push("--".into());}
                    templates.push("{path}".into());
                    (editor.program, templates, editor.terminal)
                }
                ExternalEditor::Command { program, mut args, terminal } => {
                    let program = locate(OsStr::new(&program)).context("Configured editor executable was not found or is not executable")?;
                    if !args.iter().any(|arg| arg.contains("{path}")) {args.push("{path}".into());}
                    (program, args, terminal)
                }
            };
            let args = templates.iter().map(|arg| substitute(arg, &target, &repository)).collect();
            Ok(EditorLaunch {program, args, directory:repository, terminal})
        }).await
    }
    /// Launch a GUI editor without holding a repository lock for the editing session.
    pub async fn open_editor(
        &self,
        path: Option<GitPath>,
        configuration: ExternalEditor,
    ) -> Result<Value> {
        let plan = self.editor_launch(path, configuration).await?;
        launch_editor(plan).await
    }

    /// Launch an explicitly selected custom editor for a confined repository target.
    pub async fn open_desktop_custom_editor(
        &self,
        path: Option<GitPath>,
        editor: DesktopCustomEditor,
    ) -> Result<Value> {
        let repository = self.path().to_owned();
        let plan = cancellation::blocking(move || {
            let target = editor_target(&repository, path)?;
            let invocation = desktop_editor_invocation(&editor)?;
            Ok(desktop_editor_plan(
                editor,
                invocation,
                &target,
                &repository,
            ))
        })
        .await?;
        launch_editor(plan).await
    }
}

async fn launch_editor(plan: EditorLaunch) -> Result<Value> {
    if plan.terminal {
        bail!(
            "This editor needs an interactive terminal. Run desktop-rs -C <repository> editor <file> in Termux or a terminal, then refresh and stage the edited resolution."
        );
    }
    let mut command = tokio::process::Command::new(plan.program);
    command
        .args(plan.args)
        .current_dir(plan.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    cancellation::Cancellation::current().check()?;
    let mut child = command.spawn().context("launch configured editor")?;
    match tokio::time::timeout(std::time::Duration::from_millis(200), child.wait()).await {
        Ok(Ok(status)) if !status.success() => bail!(
            "Editor exited unsuccessfully ({}). Check its executable and argument settings.",
            status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string())
        ),
        Ok(Err(error)) => return Err(error).context("wait for editor launch"),
        Ok(Ok(_)) => {
            return Ok(
                json!({"code":0,"stdout":"Editor launch completed. Save your changes, then refresh the conflict preview.","stderr":""}),
            );
        }
        Err(_) => {}
    }
    // Reap editors while the service runs. Closing the service leaves the
    // independent editor running, as ordinary desktop launches do.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(
        json!({"code":0,"stdout":"Editor launched. Save your changes, then refresh the conflict preview.","stderr":""}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn custom_editor_lookup_never_falls_back_to_shell_wrappers() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("desktop-custom-lookup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for name in ["editor.cmd", "editor.bat"] {
            let path = root.join(name);
            std::fs::write(&path, "fixture").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let search = std::env::join_paths([&root]).unwrap();
        assert!(locate_custom_in(OsStr::new("editor.exe"), &search).is_none());
        assert!(!windows_direct_executable(Path::new("editor.cmd")));
        assert!(!windows_direct_executable(Path::new("editor.bat")));
        assert!(windows_direct_executable(Path::new("editor.EXE")));
        let executable = root.join("editor.exe");
        std::fs::write(&executable, "fixture").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            locate_custom_in(OsStr::new("editor.exe"), &search),
            Some(executable)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn macos_application_is_resolved_before_the_launch_directory_changes() {
        use std::path::Component;
        let root =
            std::env::temp_dir().join(format!("desktop-custom-app-{}", uuid::Uuid::new_v4()));
        let app = root.join("Editor.app");
        std::fs::create_dir_all(&app).unwrap();
        let mut relative = PathBuf::new();
        for component in std::env::current_dir().unwrap().components() {
            if matches!(component, Component::Normal(_)) {
                relative.push("..");
            }
        }
        relative.push(app.strip_prefix("/").unwrap());
        let resolved = resolved_macos_application(&relative).unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(resolved, app.canonicalize().unwrap());
        assert_eq!(
            root.join("another-working-directory").join(&resolved),
            resolved
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn desktop_custom_editor_preserves_arguments_without_a_shell() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("desktop-custom-editor-{}", uuid::Uuid::new_v4()));
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let editor = root.join("editor script");
        let output = root.join("arguments");
        let marker = root.join("injected");
        std::fs::write(
            &editor,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                output.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let name = "target ;$() file.txt";
        std::fs::write(root.join(name), "content").unwrap();
        let repo = Repository::open(&root).await.unwrap();
        let command = DesktopCustomEditor {
            program: editor.to_str().unwrap().to_owned(),
            args: vec![
                "literal {path}".into(),
                "--open=%TARGET_PATH%".into(),
                format!("$(touch {})", marker.display()),
                "before%TARGET_PATH%after%TARGET_PATH%".into(),
                "\"%TARGET_PATH%\"".into(),
                "'%TARGET_PATH%'".into(),
            ],
            macos_app: false,
        };
        assert_eq!(
            validate_custom_integration_path(command.program.clone())
                .await
                .unwrap()["is_valid"],
            true
        );
        repo.open_desktop_custom_editor(Some(GitPath::Text(name.into())), command.clone())
            .await
            .unwrap();
        let target = root.join(name).canonicalize().unwrap();
        let target = target.to_str().unwrap();
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            format!(
                "literal {{path}}\n--open={target}\n$(touch {})\nbefore{target}after{target}\n{target}\n{target}\n",
                marker.display()
            )
        );
        assert!(!marker.exists());
        assert!(
            repo.open_desktop_custom_editor(
                Some(GitPath::Text("../outside".into())),
                command.clone()
            )
            .await
            .is_err()
        );
        let mut invalid = command;
        invalid.args = vec!["x".into(); 65];
        assert!(
            repo.open_desktop_custom_editor(None, invalid)
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn custom_integration_validation_checks_execute_access() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "desktop-custom-integration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("editor");
        std::fs::write(&file, "fixture").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            validate_custom_integration_path(file.to_str().unwrap().into())
                .await
                .unwrap()["is_valid"],
            false
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            validate_custom_integration_path(file.to_str().unwrap().into())
                .await
                .unwrap()["is_valid"],
            true
        );
        assert_eq!(
            validate_custom_integration_path(root.to_str().unwrap().into())
                .await
                .unwrap()["is_valid"],
            false
        );
        assert!(
            validate_custom_integration_path("bad\0path".into())
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_integration_arguments_use_native_quoting() {
        assert_eq!(
            parse_windows_integration_arguments(
                r#""hello world" "--file=%TARGET_PATH%" "a\"b""#.into()
            )
            .await
            .unwrap(),
            vec!["hello world", "--file=%TARGET_PATH%", "a\"b"]
        );
        assert!(
            parse_windows_integration_arguments("bad\0arguments".into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn named_editor_requires_a_known_editor_and_confined_file() {
        let root = std::env::temp_dir().join(format!("desktop-editor-{}", uuid::Uuid::new_v4()));
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let repo = Repository::open(&root).await.unwrap();
        std::fs::write(root.join("file.txt"), "test").unwrap();
        let named = ExternalEditor::Named {
            name: "not-an-allowlisted-editor".into(),
        };
        assert!(
            repo.editor_launch(Some(GitPath::Text("file.txt".into())), named.clone())
                .await
                .is_err()
        );
        assert!(
            repo.editor_launch(Some(GitPath::Text("../outside".into())), named)
                .await
                .is_err()
        );
        assert!(
            ExternalEditor::Named {
                name: "invalid\nname".into()
            }
            .validate()
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
