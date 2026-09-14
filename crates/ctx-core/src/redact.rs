//! # redact
//!
//! Pre-ingestion secret scrubber for `ContextEvent` payloads.
//!
//! ## Why Pre-Ingestion?
//!
//! Secrets must be scrubbed **before** an event is placed into the ring buffer.
//! Once bytes reach SQLite — even in WAL mode — they may be snapshotted by
//! other readers or appear in SQLite journal files. The only safe guarantee is
//! zero secrets ever written to the `ctx.db` file.
//!
//! ## Detected Patterns
//!
//! | Category | Pattern | Example |
//! |---|---|---|
//! | GitHub PAT | `ghp_[A-Za-z0-9]{36}` | `ghp_aBcDeF...` |
//! | GitHub OAuth | `gho_[A-Za-z0-9]{36}` | `gho_aBcDeF...` |
//! | GitHub Actions | `ghs_[A-Za-z0-9]{36}` | `ghs_aBcDeF...` |
//! | GitHub Fine-grained | `github_pat_[A-Za-z0-9_]{82}` | |
//! | AWS Access Key | `AKIA[0-9A-Z]{16}` | `AKIAIOSFODNN7EXAMPLE` |
//! | AWS Secret | `AWS_SECRET_ACCESS_KEY\s*=\s*\S+` | |
//! | OpenAI Key | `sk-[a-zA-Z0-9]{48}` | `sk-abc123...` |
//! | Anthropic Key | `sk-ant-api[0-9]{2}-[A-Za-z0-9_-]{93}` | |
//! | Private Key PEM | `-----BEGIN .{0,30} PRIVATE KEY-----` | |
//! | Bearer in URLs | `Bearer [A-Za-z0-9\-._~+/]+=*` | HTTP Auth header |
//! | Generic API var | `(API_KEY|SECRET|TOKEN|PASSWORD)\s*=\s*['"]?\S+` | `.env` style |
//!
//! ## Performance
//!
//! The `SecretScrubber` is constructed once at daemon startup (or first use via
//! `OnceLock`) by compiling all patterns into a single `regex::RegexSet` for
//! fast boolean detection and individual `regex::Regex` objects for replacement.
//!
//! **Fast path:** If the `RegexSet` does not match, no replacement regex is
//! ever consulted and no allocation occurs beyond the initial `is_match` call.

use std::sync::OnceLock;

use regex::{Regex, RegexSet};

// =============================================================================
// Global scrubber singleton
// =============================================================================

/// Global `SecretScrubber`, initialized once on first use.
///
/// Access via [`scrubber()`].
static GLOBAL_SCRUBBER: OnceLock<SecretScrubber> = OnceLock::new();

/// Return a reference to the global [`SecretScrubber`].
///
/// Initializes the scrubber on first call (compiles all regex patterns).
/// Subsequent calls are lock-free reads.
///
/// # Panics
///
/// Panics if any built-in pattern fails to compile (should never happen
/// with the hardcoded patterns below).
pub fn scrubber() -> &'static SecretScrubber {
    GLOBAL_SCRUBBER.get_or_init(SecretScrubber::new)
}

// =============================================================================
// SecretScrubber
// =============================================================================

/// Compiled secret-detection and redaction engine.
///
/// Build once and reuse. All fields are `Send + Sync`.
pub struct SecretScrubber {
    /// Fast multi-pattern matcher for the "does this text contain any secret?" check.
    detection_set: RegexSet,
    /// Individual patterns with their replacement labels.
    /// Order must match `detection_set`'s pattern order.
    replacements: Vec<(&'static str, Regex)>,
}

impl SecretScrubber {
    /// Build the scrubber by compiling all patterns.
    ///
    /// # Panics
    ///
    /// Panics if a built-in pattern is invalid (compile-time logic error).
    pub fn new() -> Self {
        // (label, pattern_str)
        // Labels appear as the replacement token in stored content.
        let patterns: &[(&str, &str)] = &[
            // GitHub tokens
            ("GITHUB_TOKEN", r"ghp_[A-Za-z0-9]{36,}"),
            ("GITHUB_TOKEN", r"gho_[A-Za-z0-9]{36,}"),
            ("GITHUB_TOKEN", r"ghs_[A-Za-z0-9]{36,}"),
            ("GITHUB_TOKEN", r"github_pat_[A-Za-z0-9_]{82,}"),
            // AWS
            ("AWS_ACCESS_KEY", r"AKIA[0-9A-Z]{16}"),
            (
                "AWS_SECRET",
                r"(?i)aws[_\s]secret[_\s]access[_\s]key\s*[=:]\s*\S+",
            ),
            // OpenAI
            ("OPENAI_KEY", r"sk-[a-zA-Z0-9]{48,}"),
            // Anthropic
            ("ANTHROPIC_KEY", r"sk-ant-api[0-9]{2}-[A-Za-z0-9_\-]{93,}"),
            // Private key PEM headers
            ("PRIVATE_KEY", r"-----BEGIN [A-Z ]{0,30}PRIVATE KEY-----"),
            // Bearer tokens in HTTP headers / curl commands
            ("BEARER_TOKEN", r"Bearer [A-Za-z0-9\-._~+/]{20,}={0,2}"),
            // Generic .env-style secret assignments
            (
                "SECRET_VALUE",
                r#"(?i)(API_KEY|SECRET_KEY|ACCESS_SECRET|TOKEN|PASSWORD|PASSWD|AUTH_TOKEN)\s*[=:]\s*['"]?[A-Za-z0-9\-._~+/@!#$%^&*]{8,}['"]?"#,
            ),
        ];

        let pattern_strings: Vec<&str> = patterns.iter().map(|(_, p)| *p).collect();

        let detection_set = RegexSet::new(&pattern_strings)
            .expect("built-in secret detection patterns must compile");

        let replacements: Vec<(&'static str, Regex)> = patterns
            .iter()
            .map(|(label, pattern)| {
                let re = Regex::new(pattern).expect("built-in pattern must compile");
                (*label, re)
            })
            .collect();

        Self {
            detection_set,
            replacements,
        }
    }

    /// Scrub secrets from `input`.
    ///
    /// Returns `(scrubbed_content, was_redacted)`.
    ///
    /// If no pattern matches, returns a clone of `input` with `was_redacted = false`
    /// (the `RegexSet` check is O(n) with respect to input length but avoids all
    /// replacement overhead).
    ///
    /// If one or more patterns match, each is replaced with `[REDACTED_<LABEL>]`.
    pub fn scrub(&self, input: &str) -> (String, bool) {
        // Fast path: single-pass check against all patterns simultaneously
        if !self.detection_set.is_match(input) {
            return (input.to_string(), false);
        }

        // One or more secrets detected — apply all applicable replacements
        let mut output = input.to_string();
        let mut was_redacted = false;

        for (label, re) in &self.replacements {
            if re.is_match(&output) {
                let replacement = format!("[REDACTED_{label}]");
                let replaced = re.replace_all(&output, replacement.as_str());
                if replaced != output.as_str() {
                    output = replaced.into_owned();
                    was_redacted = true;
                }
            }
        }

        (output, was_redacted)
    }
}

impl Default for SecretScrubber {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> SecretScrubber {
        SecretScrubber::new()
    }

    #[test]
    fn test_plain_text_fast_path() {
        let (out, redacted) = s().scrub("cargo build --release passed in 2.3s");
        assert!(!redacted);
        assert_eq!(out, "cargo build --release passed in 2.3s");
    }

    #[test]
    fn test_github_pat_detected() {
        let input =
            "git clone https://ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcde12345@github.com/org/repo";
        let (out, redacted) = s().scrub(input);
        assert!(redacted);
        assert!(!out.contains("ghp_"), "PAT should be removed, got: {out}");
        assert!(out.contains("[REDACTED_GITHUB_TOKEN]"));
    }

    #[test]
    fn test_aws_access_key_detected() {
        let input = "export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let (out, redacted) = s().scrub(input);
        assert!(redacted);
        assert!(
            !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "AWS key should be removed, got: {out}"
        );
    }

    #[test]
    fn test_openai_key_detected() {
        let key = format!("sk-{}", "a".repeat(48));
        let input = format!("OPENAI_API_KEY={key}");
        let (out, redacted) = s().scrub(&input);
        assert!(redacted);
        assert!(
            !out.contains(&key),
            "OpenAI key should be removed, got: {out}"
        );
    }

    #[test]
    fn test_private_key_header_detected() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...";
        let (out, redacted) = s().scrub(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED_PRIVATE_KEY]"));
    }

    #[test]
    fn test_bearer_token_detected() {
        let input = "curl -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.test'";
        let (out, redacted) = s().scrub(input);
        assert!(redacted);
        assert!(
            !out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"),
            "Bearer token should be removed, got: {out}"
        );
    }

    #[test]
    fn test_generic_env_secret_detected() {
        let input = "API_KEY=super_secret_value_12345";
        let (out, redacted) = s().scrub(input);
        assert!(redacted);
        assert!(
            !out.contains("super_secret_value_12345"),
            "Secret value should be removed, got: {out}"
        );
    }

    #[test]
    fn test_multiple_secrets_all_redacted() {
        let input = format!(
            "export GITHUB={} AWS=AKIAIOSFODNN7EXAMPLE",
            "ghp_".to_string() + &"x".repeat(36)
        );
        let (out, redacted) = s().scrub(&input);
        assert!(redacted);
        assert!(!out.contains("ghp_"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_was_redacted_false_for_clean_content() {
        let inputs = [
            "test result: ok. 14 passed",
            "fn main() { println!(\"Hello\"); }",
            "cargo clippy --workspace -- -D warnings",
            "git commit -m 'Fix auth middleware'",
        ];
        for input in inputs {
            let (_, redacted) = s().scrub(input);
            assert!(!redacted, "Should not redact clean content: '{input}'");
        }
    }

    #[test]
    fn test_global_scrubber_singleton() {
        // Should not panic; subsequent calls return the same instance
        let s1 = scrubber();
        let s2 = scrubber();
        // Both point to the same static allocation
        assert!(std::ptr::eq(std::ptr::from_ref(s1), std::ptr::from_ref(s2)));
    }
}
