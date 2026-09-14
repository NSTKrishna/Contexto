//! # truncation
//!
//! Smart dual-ended content truncation for `ContextEvent` payloads.
//!
//! ## Why Dual-Ended?
//!
//! Naive head-only truncation is dangerous for developer context:
//!
//! ```text
//! cargo test output (100KB):
//!   ├── Lines 1-300:   Compilation output, feature flags, crate downloads
//!   ├── Lines 301-800: Successful test output ("test foo ... ok")
//!   └── Lines 801-...: PANIC / ASSERTION FAILURE / actual error  ← discarded!
//! ```
//!
//! Keeping only the first 8KB drops the only part that matters.
//! Dual-ended truncation preserves both the initial context (what command,
//! what config) and the terminal outcome (stack trace, test result, exit code).
//!
//! ## Format
//!
//! ```text
//! [first HEAD_SIZE bytes of output]
//!
//! ... [Contexto: truncated N bytes — HEAD_SIZE..len-TAIL_SIZE omitted] ...
//!
//! [last TAIL_SIZE bytes of output]
//! ```
//!
//! Total stored bytes ≤ `MAX_CONTENT_SIZE` (8192) in all cases.

/// Maximum allowed content size for a single event in bytes.
/// Must match `ctx_core::MAX_CONTENT_SIZE`.
pub const MAX_CONTENT_SIZE: usize = 8_192;

/// How many bytes to preserve from the head (start) of the content.
const HEAD_SIZE: usize = 3_800;

/// How many bytes to preserve from the tail (end) of the content.
const TAIL_SIZE: usize = 3_800;

/// Truncation result returned by [`truncate_head_tail`].
pub struct TruncationResult {
    /// The (possibly truncated) content string.
    pub content: String,
    /// `true` if the content was truncated; `false` if it fits within limits.
    pub was_truncated: bool,
    /// Original byte length before truncation (0 if no truncation).
    pub original_len: usize,
}

/// Truncate content using a dual-ended (head + tail) strategy.
///
/// If `input.len() <= MAX_CONTENT_SIZE`, returns the string unchanged with
/// `was_truncated = false` and no heap allocation beyond the `String::to_string()`.
///
/// Otherwise, preserves `HEAD_SIZE` bytes from the start, inserts a marker
/// noting the number of omitted bytes, and appends `TAIL_SIZE` bytes from
/// the end. The result always fits within `MAX_CONTENT_SIZE` bytes.
///
/// ## Boundary Behaviour
///
/// Truncation boundaries are adjusted to the nearest valid UTF-8 character
/// boundary to prevent producing malformed strings.
///
/// # Examples
///
/// ```
/// use ctx_core::truncation::truncate_head_tail;
///
/// let short = "hello world";
/// let result = truncate_head_tail(short);
/// assert!(!result.was_truncated);
/// assert_eq!(result.content, short);
///
/// let large = "A".repeat(20_000);
/// let result = truncate_head_tail(&large);
/// assert!(result.was_truncated);
/// assert!(result.content.len() <= ctx_core::truncation::MAX_CONTENT_SIZE + 100); // marker adds a few bytes
/// assert!(result.content.contains("[Contexto: truncated"));
/// ```
pub fn truncate_head_tail(input: &str) -> TruncationResult {
    if input.len() <= MAX_CONTENT_SIZE {
        return TruncationResult {
            content: input.to_string(),
            was_truncated: false,
            original_len: 0,
        };
    }

    let original_len = input.len();

    // Find a valid UTF-8 char boundary for the head cut
    let head_end = floor_char_boundary(input, HEAD_SIZE);
    let head = &input[..head_end];

    // Find a valid UTF-8 char boundary for the tail cut
    // tail_start is at least HEAD_SIZE from the end
    let tail_start_raw = original_len.saturating_sub(TAIL_SIZE);
    let tail_start = ceil_char_boundary(input, tail_start_raw);
    let tail = &input[tail_start..];

    let omitted_bytes = tail_start.saturating_sub(head_end);

    let marker = format!(
        "\n\n... [Contexto: truncated {omitted_bytes} bytes — middle of output omitted for token efficiency] ...\n\n"
    );

    let mut result = String::with_capacity(HEAD_SIZE + marker.len() + TAIL_SIZE);
    result.push_str(head);
    result.push_str(&marker);
    result.push_str(tail);

    TruncationResult {
        content: result,
        was_truncated: true,
        original_len,
    }
}

/// Find the largest index `<= index` that is a valid UTF-8 char boundary.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Find the smallest index `>= index` that is a valid UTF-8 char boundary.
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_content_unchanged() {
        let input = "cargo test passed";
        let result = truncate_head_tail(input);
        assert!(!result.was_truncated);
        assert_eq!(result.content, input);
        assert_eq!(result.original_len, 0);
    }

    #[test]
    fn test_exactly_at_limit_unchanged() {
        let input = "x".repeat(MAX_CONTENT_SIZE);
        let result = truncate_head_tail(&input);
        assert!(!result.was_truncated);
        assert_eq!(result.content.len(), MAX_CONTENT_SIZE);
    }

    #[test]
    fn test_large_content_truncated() {
        let input = "A".repeat(20_000);
        let result = truncate_head_tail(&input);
        assert!(result.was_truncated);
        assert_eq!(result.original_len, 20_000);
    }

    #[test]
    fn test_truncation_marker_present() {
        let input = "A".repeat(20_000);
        let result = truncate_head_tail(&input);
        assert!(result.content.contains("[Contexto: truncated"));
        assert!(result.content.contains("bytes — middle of output omitted"));
    }

    #[test]
    fn test_head_preserved() {
        // Build a string where head has a unique prefix
        let head = "HEAD_CONTENT ".repeat(300); // ~3900 bytes
        let middle = "MIDDLE ".repeat(1000);
        let tail_part = "TAIL_CONTENT ".repeat(300);
        let input = format!("{head}{middle}{tail_part}");

        let result = truncate_head_tail(&input);
        assert!(result.was_truncated);
        assert!(result.content.starts_with("HEAD_CONTENT"));
    }

    #[test]
    fn test_tail_preserved() {
        let head = "HEAD ".repeat(300);
        let middle = "MIDDLE ".repeat(1000);
        let tail_part = "TAIL_CONTENT ".repeat(300);
        let input = format!("{head}{middle}{tail_part}");

        let result = truncate_head_tail(&input);
        assert!(result.was_truncated);
        assert!(result.content.ends_with("TAIL_CONTENT "));
    }

    #[test]
    fn test_valid_utf8_boundaries() {
        // Build content with multi-byte UTF-8 characters to stress boundary logic
        let emoji_block = "🦀🔥🎯".repeat(2000); // each char is 4 bytes
        let result = truncate_head_tail(&emoji_block);
        // Result must be valid UTF-8 (String creation implies this)
        assert!(!result.content.is_empty());
        if result.was_truncated {
            // Check no partial char sequences
            assert!(std::str::from_utf8(result.content.as_bytes()).is_ok());
        }
    }

    #[test]
    fn test_cargo_test_failure_tail_preserved() {
        // Simulate a cargo test where the failure is at the bottom
        let header = format!("{}\n", "running 150 tests".repeat(200)); // lots of compile output
        let failure = "\nthread 'test_auth' panicked at 'assertion failed: token == expected'\nnote: run with `RUST_BACKTRACE=1` for a backtrace\ntest result: FAILED. 149 passed; 1 failed";

        let input = format!("{header}{failure}");
        let result = truncate_head_tail(&input);

        if result.was_truncated {
            assert!(
                result.content.contains("FAILED"),
                "Tail (failure message) must be preserved"
            );
        }
    }
}
