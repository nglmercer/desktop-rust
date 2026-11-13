//! Incremental redaction for credential-scoped process output.
use std::collections::VecDeque;

struct Matcher {
    secret: Vec<u8>,
    prefix: Vec<usize>,
    matched: usize,
}

impl Matcher {
    fn new(secret: Vec<u8>) -> Self {
        let mut prefix = vec![0; secret.len()];
        let mut matched = 0;
        for index in 1..secret.len() {
            while matched > 0 && secret[index] != secret[matched] {
                matched = prefix[matched - 1];
            }
            if secret[index] == secret[matched] {
                matched += 1;
            }
            prefix[index] = matched;
        }
        Self {
            secret,
            prefix,
            matched: 0,
        }
    }

    fn push(&mut self, byte: u8) -> bool {
        while self.matched > 0 && byte != self.secret[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if byte == self.secret[self.matched] {
            self.matched += 1;
        }
        self.matched == self.secret.len()
    }
}

/// Holds unresolved output until the operation's credential set is known.
/// Once active, retains only bytes that could begin a secret. Credential values
/// are ASCII graphic, so newline-delimited replacement breaks cross-boundary
/// matches. If a short credential appears in the marker, use just a newline.
pub(crate) struct StreamRedactor {
    matchers: Option<Vec<Matcher>>,
    unknown: Vec<u8>,
    pending: VecDeque<u8>,
    replacement: &'static [u8],
    blocked: bool,
}

impl StreamRedactor {
    pub(crate) fn new() -> Self {
        Self {
            matchers: None,
            unknown: Vec::new(),
            pending: VecDeque::new(),
            replacement: b"\n[redacted]\n",
            blocked: false,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8], secrets: Option<Vec<Vec<u8>>>) -> Vec<u8> {
        if self.blocked {
            return Vec::new();
        }
        if self.matchers.is_none() {
            let Some(secrets) = secrets else {
                if self.unknown.len().saturating_add(bytes.len()) > 2 * 1024 * 1024 {
                    self.unknown.clear();
                    self.blocked = true;
                    return Vec::new();
                }
                self.unknown.extend_from_slice(bytes);
                return Vec::new();
            };
            if secrets.iter().any(|secret| {
                secret.is_empty() || secret.iter().any(|byte| !byte.is_ascii_graphic())
            }) {
                self.unknown.clear();
                self.blocked = true;
                return Vec::new();
            }
            let marker = b"[redacted]";
            if secrets.iter().any(|secret| {
                !secret.is_empty() && marker.windows(secret.len()).any(|window| window == secret)
            }) {
                self.replacement = b"\n";
            }
            self.matchers = Some(
                secrets
                    .into_iter()
                    .filter(|secret| !secret.is_empty())
                    .map(Matcher::new)
                    .collect(),
            );
            let unknown = std::mem::take(&mut self.unknown);
            let mut result = self.process(&unknown);
            result.extend(self.process(bytes));
            return result;
        }
        self.process(bytes)
    }

    fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        let matchers = self.matchers.as_mut().expect("redaction initialized");
        let mut result = Vec::new();
        for &byte in bytes {
            self.pending.push_back(byte);
            let mut full = 0;
            let mut hold = 0;
            for matcher in matchers.iter_mut() {
                if matcher.push(byte) {
                    full = full.max(matcher.secret.len());
                }
                hold = hold.max(matcher.matched);
            }
            if full > 0 {
                let prefix = self.pending.len() - full;
                result.extend(self.pending.drain(..prefix));
                self.pending.clear();
                result.extend_from_slice(self.replacement);
                for matcher in matchers.iter_mut() {
                    matcher.matched = 0;
                }
            } else {
                result.extend(self.pending.drain(..self.pending.len() - hold));
            }
        }
        result
    }

    pub(crate) fn finish(&mut self, secrets: Vec<Vec<u8>>) -> Vec<u8> {
        let mut result = self.push(&[], Some(secrets));
        result.extend(self.pending.drain(..));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_secrets_across_chunks_and_before_credentials_are_known() {
        let secret = b"token-123".to_vec();
        let mut redactor = StreamRedactor::new();
        assert!(redactor.push(b"before token-", None).is_empty());
        let mut output = redactor.push(b"123 after ", Some(vec![secret.clone()]));
        for byte in b"token-123 end" {
            output.extend(redactor.push(&[*byte], None));
        }
        output.extend(redactor.finish(vec![secret.clone()]));
        assert_eq!(output, b"before \n[redacted]\n after \n[redacted]\n end");
        assert!(!output.windows(secret.len()).any(|window| window == secret));
    }

    #[test]
    fn handles_overlaps_and_secrets_inside_the_marker() {
        let secrets = vec![b"abcabc".to_vec(), b"red".to_vec()];
        let mut redactor = StreamRedactor::new();
        let mut output = redactor.push(b"abcab", Some(secrets.clone()));
        output.extend(redactor.push(b"c:red", None));
        output.extend(redactor.finish(secrets.clone()));
        assert_eq!(output, b"\n:\n");
        for secret in secrets {
            assert!(!output.windows(secret.len()).any(|window| window == secret));
        }
    }

    #[test]
    fn suppresses_output_if_a_secret_cannot_be_redacted_safely() {
        let mut redactor = StreamRedactor::new();
        assert!(redactor.push(b"early", None).is_empty());
        assert!(
            redactor
                .push(b" sensitive", Some(vec![b"not\nprintable".to_vec()]))
                .is_empty()
        );
        assert!(redactor.finish(vec![]).is_empty());
    }

    #[test]
    fn preserves_binary_output_and_bounds_pending_prefixes() {
        let secret = [vec![b'a'; 65_535], vec![b'b']].concat();
        let mut redactor = StreamRedactor::new();
        let mut output = redactor.push(&[0xff, 0, b'x'], Some(vec![secret.clone()]));
        output.extend(redactor.push(&vec![b'a'; 65_535], None));
        output.extend(redactor.push(b"b!", None));
        output.extend(redactor.finish(vec![secret]));
        assert_eq!(output, b"\xff\0x\n[redacted]\n!");
    }
}
