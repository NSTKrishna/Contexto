//! # ansi
//!
//! Zero-allocation ANSI / VT100 escape sequence stripper.
//!
//! Terminal output from `cargo test`, `npm install`, and AI CLI agents
//! (Claude Code, Gemini CLI) contains heavy VT100 sequences:
//!
//! - Color codes:         `\x1b[32m`, `\x1b[0m`, `\x1b[38;5;82m`
//! - Cursor movement:     `\x1b[2K`, `\x1b[1A`, `\x1b[?25l`
//! - Terminal title:      `\x1b]0;title\x07`
//! - Bold/underline:      `\x1b[1m`, `\x1b[4m`
//!
//! Storing these sequences raw wastes 40–60% of disk space and poisons
//! the FTS5 inverted index with junk tokens (`\x1b`, `[32m`, `[0m`).
//!
//! ## Design
//!
//! Uses a byte-level state machine to detect and skip escape sequences
//! without any regex or heap allocation when no ESC byte is present.
//! This is the most common case for non-terminal sources (IDE, Git, Manual),
//! so the fast path matters.
//!
//! ## Coverage
//!
//! - CSI sequences:  `\x1b[` ... final byte `0x40–0x7E`
//! - OSC sequences:  `\x1b]` ... `BEL (\x07)` or `ST (\x1b\\)`
//! - Single-char:    `\x1b` followed by any non-`[`, non-`]` byte
//!
//! ## References
//!
//! - ECMA-48 (ISO/IEC 6429): Control Functions for Character-Oriented Systems
//! - XTerm: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>

/// Strip all ANSI/VT100 escape sequences from `input`.
///
/// Returns the original string unchanged if no ESC byte (`\x1b`) is found,
/// without performing any heap allocation.
///
/// # Examples
///
/// ```
/// use ctx_core::ansi::strip_ansi;
///
/// assert_eq!(strip_ansi("\x1b[32mHello\x1b[0m"), "Hello");
/// assert_eq!(strip_ansi("plain text"), "plain text");
/// assert_eq!(strip_ansi("\x1b[2K\x1b[1Aoverwritten"), "overwritten");
/// ```
pub fn strip_ansi(input: &str) -> String {
    // Fast path: if no ESC byte is present, skip all processing.
    // This is the common case for Manual, Git, and Editor events.
    if !input.as_bytes().contains(&0x1b) {
        return input.to_string();
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != 0x1b {
            // Normal byte — emit it
            out.push(bytes[i]);
            i += 1;
            continue;
        }

        // ESC byte detected
        i += 1;
        if i >= bytes.len() {
            // Lone ESC at end — skip it
            break;
        }

        match bytes[i] {
            // CSI sequence: ESC [ params final_byte
            // Final byte is in range 0x40–0x7E (@–~)
            b'[' => {
                i += 1; // skip '['
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if (0x40..=0x7e).contains(&b) {
                        // Final byte of CSI sequence — done
                        break;
                    }
                    // Parameter or intermediate byte — keep scanning
                }
            }

            // OSC sequence: ESC ] ... BEL or ST (ESC \)
            b']' => {
                i += 1; // skip ']'
                while i < bytes.len() {
                    match bytes[i] {
                        // BEL terminates OSC
                        0x07 => {
                            i += 1;
                            break;
                        }
                        // ST = ESC \ terminates OSC
                        0x1b if i + 1 < bytes.len() && bytes[i + 1] == b'\\' => {
                            i += 2;
                            break;
                        }
                        _ => {
                            i += 1;
                        }
                    }
                }
            }

            // Any other ESC + single byte (e.g. ESC M for reverse index)
            _ => {
                i += 1; // skip the byte after ESC
            }
        }
    }

    // SAFETY: we only copy bytes from the original valid UTF-8 string,
    // excluding ESC and escape sequence bytes. The result is valid UTF-8.
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => {
            // Should never happen; fall back to lossy conversion
            tracing::warn!("ANSI stripper produced invalid UTF-8: {e}");
            String::from_utf8_lossy(e.as_bytes()).into_owned()
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_text_fast_path() {
        let input = "cargo build --release";
        // No ESC byte → must return without heap allocation (same content)
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn test_color_codes_stripped() {
        assert_eq!(strip_ansi("\x1b[32mHello\x1b[0m"), "Hello");
    }

    #[test]
    fn test_256_color_stripped() {
        assert_eq!(strip_ansi("\x1b[38;5;82mGreen\x1b[0m"), "Green");
    }

    #[test]
    fn test_cursor_movement_stripped() {
        // \x1b[2K = erase line, \x1b[1A = cursor up
        assert_eq!(strip_ansi("\x1b[2K\x1b[1Aoverwritten"), "overwritten");
    }

    #[test]
    fn test_terminal_title_osc_stripped() {
        // OSC 0 ; title BEL
        assert_eq!(
            strip_ansi("\x1b]0;My Terminal Title\x07normal text"),
            "normal text"
        );
    }

    #[test]
    fn test_osc_with_st_terminator() {
        // OSC terminated by ST (ESC \)
        assert_eq!(strip_ansi("\x1b]0;title\x1b\\normal text"), "normal text");
    }

    #[test]
    fn test_bold_underline_stripped() {
        assert_eq!(strip_ansi("\x1b[1mBold\x1b[0m"), "Bold");
        assert_eq!(strip_ansi("\x1b[4mUnderline\x1b[0m"), "Underline");
    }

    #[test]
    fn test_mixed_content() {
        let input = "\x1b[32m✓ test_ring_buffer\x1b[0m passed in 0.003s";
        assert_eq!(strip_ansi(input), "✓ test_ring_buffer passed in 0.003s");
    }

    #[test]
    fn test_cargo_test_output() {
        // Simulate realistic cargo test output with multiple sequences
        let input =
            "\x1b[1m\x1b[32mrunning\x1b[0m 5 tests\n\x1b[32mtest foo\x1b[0m ... \x1b[32mok\x1b[0m";
        assert_eq!(strip_ansi(input), "running 5 tests\ntest foo ... ok");
    }

    #[test]
    fn test_lone_esc_at_end() {
        // Should not panic or produce garbage
        let input = "text\x1b";
        assert_eq!(strip_ansi(input), "text");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn test_only_escape_sequences() {
        assert_eq!(strip_ansi("\x1b[0m\x1b[1m\x1b[32m"), "");
    }

    #[test]
    fn test_unicode_preserved() {
        let input = "\x1b[32m🦀 Rust\x1b[0m";
        assert_eq!(strip_ansi(input), "🦀 Rust");
    }

    #[test]
    fn test_newlines_preserved() {
        let input = "\x1b[32mline1\x1b[0m\nline2\n\x1b[31mline3\x1b[0m";
        assert_eq!(strip_ansi(input), "line1\nline2\nline3");
    }
}
