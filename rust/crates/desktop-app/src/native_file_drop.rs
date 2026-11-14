//! Validate paths delivered by the native webview file-drop callback.

use std::path::PathBuf;

const MAX_PATHS: usize = 256;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// Reject the whole drop rather than silently adding only part of it.
pub fn paths(paths: Vec<PathBuf>) -> Option<Vec<String>> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return None;
    }
    let mut result = Vec::with_capacity(paths.len());
    let mut bytes = 0usize;
    for path in paths {
        if !path.is_absolute() {
            return None;
        }
        let path = path.to_str()?;
        // Wry's Unix platform adapters may have already replaced invalid
        // native bytes with U+FFFD. Refuse ambiguity even though this also
        // rejects a valid filename containing that literal character.
        if path.contains('\u{fffd}') {
            return None;
        }
        bytes = bytes.checked_add(path.len())?;
        if bytes > MAX_PAYLOAD_BYTES {
            return None;
        }
        result.push(path.to_owned());
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_absolute_paths_and_preserves_order_and_unicode() {
        assert_eq!(
            paths(vec![PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/ñ")]),
            Some(vec!["/tmp/a b".into(), "/tmp/ñ".into()])
        );
    }

    #[test]
    fn refuses_empty_relative_and_oversized_drops() {
        assert_eq!(paths(vec![]), None);
        assert_eq!(
            paths(vec![PathBuf::from("/tmp/good"), PathBuf::from("relative")]),
            None
        );
        assert_eq!(paths(vec![PathBuf::from("/tmp/\u{fffd}")]), None);
        assert_eq!(paths(vec![PathBuf::from("/tmp"); MAX_PATHS + 1]), None);
        assert_eq!(
            paths(vec![PathBuf::from(format!(
                "/{}",
                "x".repeat(MAX_PAYLOAD_BYTES)
            ))]),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_non_utf8_paths_instead_of_changing_them() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::ffi::OsString::from_vec(b"/tmp/\xff".to_vec());
        assert_eq!(paths(vec![PathBuf::from(path)]), None);
    }
}
