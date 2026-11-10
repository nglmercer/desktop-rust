//! Registers this binary as the OS handler for the OAuth callback scheme,
//! the equivalent of Electron's `app.setAsDefaultProtocolClient`.
//!
//! GitHub redirects a finished sign-in to the callback URL registered for the
//! OAuth app, so the scheme is not ours to choose: the development client id
//! redirects to `x-github-desktop-dev-auth://oauth`. If another GitHub
//! Desktop install owns that scheme, the browser hands the authorization
//! code to it and this app waits forever.
//!
//! Only the *development* auth scheme is claimed. `x-github-client` and the
//! production `x-github-desktop-auth` stay with whatever handles them, so an
//! installed GitHub Desktop keeps working.

/// The callback scheme of the OAuth app this build signs in with.
pub const AUTH_SCHEME: &str = "x-github-desktop-dev-auth";

#[cfg(all(unix, not(target_os = "macos")))]
pub fn register() {
    use std::process::{Command, Stdio};

    const DESKTOP_FILE: &str = "github-desktop-rs.desktop";
    let mime = format!("x-scheme-handler/{AUTH_SCHEME}");

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(applications) = data_home().map(|dir| dir.join("applications")) else {
        return;
    };

    let entry = desktop_entry(&exe.to_string_lossy(), &mime);
    let path = applications.join(DESKTOP_FILE);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(entry.as_str()) {
        if let Err(error) =
            std::fs::create_dir_all(&applications).and_then(|_| std::fs::write(&path, &entry))
        {
            tracing::warn!(%error, path = %path.display(), "could not write the desktop entry; sign-in callbacks will not reach this app");
            return;
        }
    }

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
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    };

    let previous = current();
    if previous == DESKTOP_FILE {
        return;
    }

    // Desktop environments answer "who handles this scheme" from caches of
    // the installed desktop entries; a new entry is invisible until they are
    // rebuilt. Each tool is optional and specific to one environment.
    quiet(
        "update-desktop-database",
        &[&applications.to_string_lossy()],
    );
    quiet("xdg-mime", &["default", DESKTOP_FILE, &mime]);
    if current() != DESKTOP_FILE {
        quiet("gio", &["mime", &mime, DESKTOP_FILE]);
    }
    quiet("kbuildsycoca6", &[]);

    if current() == DESKTOP_FILE {
        tracing::info!(
            scheme = AUTH_SCHEME,
            previous = %if previous.is_empty() { "none" } else { &previous },
            "registered as the sign-in callback handler"
        );
    } else {
        tracing::warn!(
            scheme = AUTH_SCHEME,
            handler = %current(),
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

/// A desktop entry whose only job is receiving the callback URL.
#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_entry(exe: &str, mime: &str) -> String {
    // Quote per the Desktop Entry spec: inside double quotes, escape
    // `"`, `` ` ``, `$` and `\`; a literal `%` is written `%%`.
    let mut quoted = String::with_capacity(exe.len() + 2);
    for c in exe.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=GitHub Desktop (Rust)\n\
         Comment=Receives the GitHub sign-in callback\n\
         Exec=\"{quoted}\" %u\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType={mime};\n"
    )
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_the_executable_and_passes_the_url() {
        let entry = desktop_entry("/opt/my \"apps\"/100%/desktop", "x-scheme-handler/x-test");
        assert!(entry.contains("Exec=\"/opt/my \\\"apps\\\"/100%%/desktop\" %u\n"));
        assert!(entry.contains("MimeType=x-scheme-handler/x-test;\n"));
    }
}
