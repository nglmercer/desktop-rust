//! Classify common Git fetch failures without returning remote-controlled output.
//!
//! Git's output can include credentials or remote-supplied text. Only these
//! fixed identifiers cross the renderer boundary; unmatched output stays local.

fn classify_stream(output: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(output);
    if text.contains("[EPOLICYKEYAGE]")
        && text.contains("fatal: Could not read from remote repository.")
    {
        Some("ssh_key_unverified")
    } else if text.contains("fatal: Authentication failed for 'https://")
        || text.contains("fatal: Authentication failed for 'http://")
    {
        Some("https_authentication_failed")
    } else if text.contains("fatal: Authentication failed") {
        Some("ssh_authentication_failed")
    } else if text.contains("fatal: Could not read from remote repository.") {
        Some("ssh_permission_denied")
    } else if text.contains("The requested URL returned error: 403") {
        Some("https_authentication_failed")
    } else if text.contains("fatal: the remote end hung up unexpectedly")
        || text.contains("fatal: The remote end hung up unexpectedly")
    {
        Some("remote_disconnection")
    } else if text.contains("fatal: unable to access '")
        && text.contains("Failed to connect to ")
        && text.contains(": Host is down")
    {
        Some("host_down")
    } else if text.contains("fatal: repository '") && text.contains("' not found") {
        Some("https_repository_not_found")
    } else if text.contains("ERROR: Repository not found") {
        Some("ssh_repository_not_found")
    } else if text.contains("Another git process seems to be running in this repository, e.g.") {
        Some("lock_file_exists")
    } else {
        None
    }
}

pub(crate) fn classify(
    stderr: &[u8],
    stdout: &[u8],
    expected_owner: Option<&str>,
) -> Option<&'static str> {
    let kind = classify_stream(stderr).or_else(|| classify_stream(stdout));
    // Return only a fixed classification, never the remote-controlled text or
    // organization name. The renderer uses its already-known repository owner.
    let remote = String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|line| line.strip_prefix("remote: "))
        .collect::<Vec<_>>()
        .join("\n");
    let saml = expected_owner.is_some_and(|owner| {
        !owner.is_empty()
            && owner.len() <= 39
            && owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (remote.contains(&format!("'{owner}' organization has enabled SAML SSO"))
                || remote.contains(&format!("'{owner}' organization has enforced SAML SSO")))
            && remote.contains("you must re-authorize")
    });
    match (kind, saml) {
        (Some("https_authentication_failed"), true) => Some("https_saml_reauth_required"),
        (Some("ssh_authentication_failed" | "ssh_permission_denied"), true) => {
            Some("ssh_saml_reauth_required")
        }
        _ => kind,
    }
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn classifies_common_fetch_failures_without_returning_output() {
        for (message, expected) in [
            (
                "fatal: Authentication failed for 'https://token@example.com/repo'",
                "https_authentication_failed",
            ),
            ("fatal: Authentication failed", "ssh_authentication_failed"),
            (
                "fatal: Could not read from remote repository.",
                "ssh_permission_denied",
            ),
            (
                "The requested URL returned error: 403",
                "https_authentication_failed",
            ),
            (
                "fatal: the remote end hung up unexpectedly",
                "remote_disconnection",
            ),
            (
                "fatal: unable to access 'https://example.com': Failed to connect to example.com: Host is down",
                "host_down",
            ),
            (
                "fatal: repository 'https://example.com/repo' not found",
                "https_repository_not_found",
            ),
            ("ERROR: Repository not found", "ssh_repository_not_found"),
            (
                "Another git process seems to be running in this repository, e.g.",
                "lock_file_exists",
            ),
        ] {
            assert_eq!(classify(message.as_bytes(), b"", None), Some(expected));
            assert_eq!(
                classify(b"unclassified", message.as_bytes(), None),
                Some(expected)
            );
        }
        assert_eq!(classify(b"secret token", b"", None), None);
        assert_eq!(
            classify(b"remote: The 'example' organization has enabled SAML SSO.\nremote: you must re-authorize\nfatal: Authentication failed for 'https://example.com'", b"", Some("example")),
            Some("https_saml_reauth_required")
        );
        assert_eq!(
            classify(b"remote: The 'other' organization has enabled SAML SSO.\nremote: you must re-authorize\nfatal: Authentication failed for 'https://example.com'", b"", Some("example")),
            Some("https_authentication_failed")
        );
        assert_eq!(
            classify(b"The 'example' organization has enabled SAML SSO. you must re-authorize\nfatal: Authentication failed for 'https://example.com'", b"", Some("example")),
            Some("https_authentication_failed")
        );
        assert_eq!(
            classify(
                b"fatal: Authentication failed",
                b"The requested URL returned error: 403",
                None
            ),
            Some("ssh_authentication_failed")
        );
    }
}
