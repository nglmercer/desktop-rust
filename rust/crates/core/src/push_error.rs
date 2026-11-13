//! Fixed push failure classifications. Never return remote-controlled output.

pub(crate) fn classify(stderr: &[u8], stdout: &[u8]) -> Option<&'static str> {
    let stderr_text = String::from_utf8_lossy(stderr);
    let stdout_text = String::from_utf8_lossy(stdout);
    let text = format!("{stderr_text}\n{stdout_text}");
    if text.contains("GITHUB PUSH PROTECTION") && text.contains("Push cannot contain secrets") {
        Some("push_protection")
    } else if text.contains("error: GH001: ") {
        Some("push_file_too_large")
    } else if text.contains("error: GH007: Your push would publish a private email address.") {
        Some("push_private_email")
    } else if text.contains("error: GH006: Protected branch update failed for ")
        && text.contains("remote: error: At least one approved review is required")
    {
        Some("push_review_required")
    } else if text.contains("error: GH006: Protected branch update failed for ")
        && text.contains("remote: error: Cannot force-push to a protected branch")
    {
        Some("push_protected_force")
    } else if text.contains("error: GH006: Protected branch update failed for ")
        && text.contains("remote: error: Required status check ")
    {
        Some("push_required_status")
    } else if text.contains("error: GH003: Sorry, force-pushing to ") {
        Some("push_force_rejected")
    } else if text.lines().any(|line| {
        line.trim_start().starts_with("! [rejected]")
            && (line.contains("(non-fast-forward)") || line.contains("(fetch first)"))
    }) && text.contains("error: failed to push some refs")
    {
        Some("push_non_fast_forward")
    } else {
        crate::fetch_error::classify(stderr, stdout, None)
    }
}

#[cfg(test)]
mod tests {
    use super::classify;
    #[test]
    fn push_errors_are_fixed_labels_without_remote_text() {
        for (text, expected) in [
            (
                " ! [rejected] main -> main (fetch first)\nerror: failed to push some refs to 'secret'",
                "push_non_fast_forward",
            ),
            (
                "remote: error: GH007: Your push would publish a private email address.",
                "push_private_email",
            ),
            (
                "remote: GITHUB PUSH PROTECTION\nremote: Push cannot contain secrets",
                "push_protection",
            ),
            (
                "remote: error: GH006: Protected branch update failed for secret\nremote: error: Cannot force-push to a protected branch",
                "push_protected_force",
            ),
        ] {
            assert_eq!(classify(text.as_bytes(), &[]), Some(expected));
        }
    }
}
