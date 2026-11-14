//! Registers this binary as the OS handler for the OAuth callback scheme,
//! the equivalent of Electron's `app.setAsDefaultProtocolClient`.
//!
//! GitHub redirects a finished sign-in to the callback URL registered for the
//! OAuth app, so the scheme is not ours to choose: the development client id
//! redirects to `x-github-desktop-dev-auth://oauth`. If another GitHub
//! Desktop install owns that scheme, the browser hands the authorization
//! code to it and this app waits forever.
//!
//! The renderer records the scheme corresponding to its OAuth credentials in
//! its bundle. A handler is registered only when no other app already handles
//! that scheme; existing associations require an explicit user choice.

/// The callback scheme selected when the embedded renderer was built.
#[cfg(all(unix, not(target_os = "macos")))]
const AUTH_SCHEME: &str = include_str!(concat!(
    env!("DESKTOP_EMBEDDED_UI_DIR"),
    "/desktop-auth-scheme"
));

#[cfg(all(unix, not(target_os = "macos")))]
fn valid_scheme(value: &str) -> Option<&'static str> {
    match value {
        "x-github-desktop-dev-auth" => Some("x-github-desktop-dev-auth"),
        "x-github-desktop-auth" => Some("x-github-desktop-auth"),
        _ => None,
    }
}

/// Follow the renderer's disk/embedded selection, refusing incomplete disk metadata.
#[cfg(all(unix, not(target_os = "macos")))]
fn scheme_for_renderer(embedded: &str, disk: Option<&std::path::Path>) -> Option<&'static str> {
    if let Some(directory) = disk {
        if std::fs::read(directory.join("index.html")).is_ok() {
            let scheme = std::fs::read_to_string(directory.join("desktop-auth-scheme")).ok()?;
            return valid_scheme(&scheme);
        }
    }
    valid_scheme(embedded)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_file(scheme: &str) -> &'static str {
    if scheme == "x-github-desktop-auth" {
        "github-desktop-rs-production.desktop"
    } else {
        "github-desktop-rs-dev-v2.desktop"
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn should_register(previous: &str) -> bool {
    previous.is_empty()
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn register() {
    #[cfg(not(feature = "packaged-ui"))]
    let disk = if cfg!(debug_assertions) || std::env::var_os("DESKTOP_SERVE_FROM_DISK").is_some() {
        Some(std::path::Path::new(env!("DESKTOP_EMBEDDED_UI_DIR")))
    } else {
        None
    };
    #[cfg(feature = "packaged-ui")]
    let disk = None;
    let Some(scheme) = scheme_for_renderer(AUTH_SCHEME, disk) else {
        tracing::warn!(
            "renderer OAuth callback metadata is missing or invalid; leaving associations unchanged"
        );
        return;
    };
    register_scheme(scheme);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn register_scheme(scheme: &str) {
    use std::process::{Command, Stdio};

    let desktop_file = desktop_file(scheme);
    let mime = format!("x-scheme-handler/{scheme}");

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(applications) = data_home().map(|dir| dir.join("applications")) else {
        return;
    };

    let quiet = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let current = || {
        Command::new("xdg-mime")
            .args(["query", "default", &mime])
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|out| out.trim().to_string())
    };

    let Some(previous) = current() else {
        tracing::warn!(
            scheme,
            "could not inspect the sign-in callback handler; leaving associations unchanged"
        );
        return;
    };
    let Some(exe) = exe.to_str() else {
        tracing::warn!("cannot register a non-UTF-8 executable path");
        return;
    };
    let Ok(data) = desktop_core::AppPaths::detect()
        .and_then(|paths| paths.user_data.canonicalize().map_err(Into::into))
    else {
        tracing::warn!("cannot resolve the data directory for sign-in callbacks");
        return;
    };
    let Some(data) = data.to_str() else {
        tracing::warn!("cannot register a non-UTF-8 data directory");
        return;
    };
    let Some(entry) = desktop_entry(exe, data, &mime) else {
        tracing::warn!("cannot register paths containing control characters");
        return;
    };
    let path = applications.join(desktop_file);
    if let Err(error) =
        std::fs::create_dir_all(&applications).and_then(|_| publish_entry(&path, &entry))
    {
        tracing::warn!(%error, path = %path.display(), "could not safely write the desktop entry; sign-in callbacks may not reach this app");
        return;
    }

    if !should_register(&previous) {
        if previous != desktop_file {
            tracing::warn!(scheme, handler = %previous, candidate = desktop_file,
                "another application handles the sign-in callback; select the candidate explicitly to sign in here");
        }
        return;
    }

    // Desktop environments answer "who handles this scheme" from caches of
    // the installed desktop entries; a new entry is invisible until they are
    // rebuilt. Each tool is optional and specific to one environment.
    quiet(
        "update-desktop-database",
        &[&applications.to_string_lossy()],
    );
    if current().as_deref() != Some(previous.as_str()) {
        tracing::warn!(
            scheme,
            "the sign-in callback handler changed during registration; leaving it unchanged"
        );
        return;
    }
    quiet("xdg-mime", &["default", desktop_file, &mime]);
    if current().as_deref() == Some("") {
        quiet("gio", &["mime", &mime, desktop_file]);
    }
    quiet("kbuildsycoca6", &[]);

    if current().as_deref() == Some(desktop_file) {
        tracing::info!(
            scheme,
            previous = %if previous.is_empty() { "none" } else { &previous },
            "registered as the sign-in callback handler"
        );
    } else {
        tracing::warn!(
            scheme,
            handler = %current().unwrap_or_else(|| "unknown".into()),
            "could not become the sign-in callback handler; the browser will hand the sign-in to that application instead"
        );
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn register() {
    // macOS claims schemes through the app bundle's Info.plist and Windows
    // through the installer's registry keys; neither is packaged yet.
}

#[cfg(all(unix, not(target_os = "macos")))]
fn data_home() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
}

/// Apply Exec argument quoting before desktop-file string escaping.
#[cfg(all(unix, not(target_os = "macos")))]
fn quote_exec_arg(value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    // Quote per the Desktop Entry spec: inside double quotes, escape
    // `"`, `` ` ``, `$` and `\`; a literal `%` is written `%%`.
    let mut quoted = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    Some(format!("\"{quoted}\""))
}

/// Apply both Exec quoting and the desktop file's string-value escaping.
#[cfg(all(unix, not(target_os = "macos")))]
fn quote_arg(value: &str) -> Option<String> {
    Some(quote_exec_arg(value)?.replace('\\', "\\\\"))
}

/// A desktop entry that preserves the active profile for callback delivery.
#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_entry(exe: &str, data: &str, mime: &str) -> Option<String> {
    let exe = quote_arg(exe)?;
    let data = quote_arg(&format!("DESKTOP_USER_DATA={data}"))?;
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=GitHub Desktop (Rust)\n\
         Comment=Receives the GitHub sign-in callback\n\
         Exec=/usr/bin/env {data} {exe} %u\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType={mime};\n\
         X-GitHub-Desktop-Rust-Managed=true\n"
    ))
}

/// Refuse unrelated files and publish through a new file without following links.
#[cfg(all(unix, not(target_os = "macos")))]
fn publish_entry(path: &std::path::Path, entry: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Write};
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => {
            let previous = std::fs::read_to_string(path)?;
            if previous == entry {
                return Ok(());
            }
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "desktop entry belongs to a different installation or profile",
            ));
        }
        Ok(_) => {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "desktop entry is not a regular file",
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = path.with_file_name(format!(".desktop-rust-{}.tmp", uuid::Uuid::new_v4()));
    let mut created = false;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        created = true;
        file.write_all(entry.as_bytes())?;
        drop(file);
        std::fs::hard_link(&temporary, path)?;
        Ok(())
    })();
    if created {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_the_executable_and_passes_the_url() {
        let entry = desktop_entry(
            "/opt/my \"apps\"/100%/desktop",
            "/tmp/user data",
            "x-scheme-handler/x-test",
        )
        .unwrap();
        assert!(entry.contains(r#"Exec=/usr/bin/env "DESKTOP_USER_DATA=/tmp/user data" "/opt/my \\"apps\\"/100%%/desktop" %u"#));
        assert!(entry.contains("MimeType=x-scheme-handler/x-test;\n"));
        assert!(desktop_entry("/tmp/\ninjected", "/tmp/data", "x-test").is_none());
        assert!(desktop_entry("/tmp/app", "/tmp/\rdata", "x-test").is_none());
    }

    #[test]
    fn gio_launch_preserves_special_characters_and_profile() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        if !Command::new("gio")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            eprintln!("gio is unavailable; skipping desktop launcher integration test");
            return;
        }

        struct TempDir(std::path::PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = TempDir(
            std::env::temp_dir().join(format!("desktop-protocol-gio-{}", uuid::Uuid::new_v4())),
        );
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root.0)
            .unwrap();
        let executable = root.0.join("app $with \"quotes\" \\ slash `tick` 100%");
        let profile = root.0.join("profile $with \"quotes\" \\ slash `tick` 100%");
        let output = root.0.join("captured");
        let desktop = root.0.join("fixture.desktop");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$DESKTOP_USER_DATA\" \"$1\" > \"$DESKTOP_TEST_CAPTURE\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let entry = desktop_entry(
            executable.to_str().unwrap(),
            profile.to_str().unwrap(),
            "x-scheme-handler/x-github-desktop-auth",
        )
        .unwrap();
        std::fs::write(&desktop, entry).unwrap();
        let url = "x-github-desktop-auth://oauth/?code=synthetic&state=fixture";
        let launched = Command::new("gio")
            .arg("launch")
            .arg(&desktop)
            .arg(url)
            .env("DESKTOP_TEST_CAPTURE", &output)
            .env("XDG_DATA_HOME", root.0.join("data"))
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_CACHE_HOME", root.0.join("cache"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", root.0.join("no-dbus").display()),
            )
            .output()
            .unwrap();
        assert!(
            launched.status.success(),
            "{}",
            String::from_utf8_lossy(&launched.stderr)
        );
        let expected = format!("{}\n{url}\n", profile.display());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::read_to_string(&output).ok().as_deref() == Some(expected.as_str()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "launcher did not preserve profile and callback: {:?}",
                std::fs::read_to_string(&output)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn does_not_take_an_existing_handler() {
        let prod = "x-github-desktop-auth";
        let dev = "x-github-desktop-dev-auth";
        let prod_file = desktop_file(prod);
        assert_ne!(prod_file, desktop_file(dev));
        assert!(should_register(""));
        assert!(!should_register("another.desktop"));
        assert!(!should_register(prod_file));
        assert!(!should_register(desktop_file(dev)));
    }

    #[test]
    fn served_disk_bundle_selects_its_own_scheme() {
        let root =
            std::env::temp_dir().join(format!("desktop-protocol-bundle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let embedded = "x-github-desktop-dev-auth";
        assert_eq!(scheme_for_renderer(embedded, Some(&root)), Some(embedded));
        std::fs::write(root.join("index.html"), "fixture").unwrap();
        assert_eq!(scheme_for_renderer(embedded, Some(&root)), None);
        std::fs::write(root.join("desktop-auth-scheme"), "x-github-desktop-auth").unwrap();
        assert_eq!(
            scheme_for_renderer(embedded, Some(&root)),
            Some("x-github-desktop-auth")
        );
        std::fs::write(root.join("desktop-auth-scheme"), "invalid").unwrap();
        assert_eq!(scheme_for_renderer(embedded, Some(&root)), None);
        assert_eq!(scheme_for_renderer(embedded, None), Some(embedded));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_foreign_entries_and_symlinks() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("desktop-protocol-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let destination = root.join("handler.desktop");
        let target = root.join("target");
        std::fs::write(&target, "keep target").unwrap();
        symlink(&target, &destination).unwrap();
        assert!(publish_entry(&destination, "new").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep target");
        std::fs::remove_file(&destination).unwrap();
        std::fs::write(&destination, "foreign").unwrap();
        assert!(publish_entry(&destination, "new").is_err());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "foreign");
        std::fs::write(
            &destination,
            "X-GitHub-Desktop-Rust-Managed=true\nother profile",
        )
        .unwrap();
        assert!(publish_entry(&destination, "new").is_err());
        std::fs::write(&destination, "legacy").unwrap();
        assert!(publish_entry(&destination, "new").is_err());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "legacy");
        std::fs::remove_file(&destination).unwrap();
        publish_entry(&destination, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn xdg_registration_preserves_other_handlers_and_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        if std::env::var_os("DESKTOP_PROTOCOL_TEST_CHILD").is_some() {
            register_scheme(&std::env::var("DESKTOP_PROTOCOL_TEST_SCHEME").unwrap());
            return;
        }
        let root =
            std::env::temp_dir().join(format!("desktop-protocol-xdg-{}", uuid::Uuid::new_v4()));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let xdg = bin.join("xdg-mime");
        std::fs::write(&xdg, "#!/bin/sh\ncase \"$1\" in\nquery) [ \"$FAKE_QUERY_FAIL\" = 1 ] && exit 1; if [ -f \"$HANDLER_STORE\" ]; then while IFS= read -r line; do printf '%s\\n' \"$line\"; done < \"$HANDLER_STORE\"; fi;;\ndefault) printf '%s\\n' \"$2\" > \"$HANDLER_STORE\";;\nesac\n").unwrap();
        std::fs::set_permissions(&xdg, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = root.join("handler");
        let run = |fail: bool, scheme: &str| {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "protocol::tests::xdg_registration_preserves_other_handlers_and_fails_closed",
                ])
                .env("DESKTOP_PROTOCOL_TEST_CHILD", "1")
                .env("DESKTOP_PROTOCOL_TEST_SCHEME", scheme)
                .env("DESKTOP_USER_DATA", root.join("profile with space"))
                .env("XDG_DATA_HOME", root.join("data"))
                .env("PATH", &bin)
                .env("HANDLER_STORE", &store)
                .env("FAKE_QUERY_FAIL", if fail { "1" } else { "0" })
                .status()
                .unwrap();
            assert!(status.success());
        };
        std::fs::write(&store, "other.desktop\n").unwrap();
        run(true, "x-github-desktop-auth");
        assert_eq!(std::fs::read_to_string(&store).unwrap(), "other.desktop\n");
        assert!(!root.join("data/applications").exists());
        run(false, "x-github-desktop-auth");
        assert_eq!(std::fs::read_to_string(&store).unwrap(), "other.desktop\n");
        let entry = root.join("data/applications/github-desktop-rs-production.desktop");
        let text = std::fs::read_to_string(entry).unwrap();
        assert!(text.contains("profile with space"));
        assert!(text.contains("x-scheme-handler/x-github-desktop-auth;"));
        std::fs::write(&store, "").unwrap();
        run(false, "x-github-desktop-auth");
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            "github-desktop-rs-production.desktop\n"
        );
        std::fs::write(&store, "github-desktop-rs.desktop\n").unwrap();
        run(false, "x-github-desktop-dev-auth");
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            "github-desktop-rs.desktop\n"
        );
        assert!(
            root.join("data/applications/github-desktop-rs-dev-v2.desktop")
                .is_file()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
