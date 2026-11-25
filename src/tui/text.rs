//! Small helpers for putting untrusted text on screen.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Make text from a database safe to print on one line: line breaks become
/// a visible mark and other control characters (which could move the cursor
/// or change colours) become spaces. Reads at most `max_chars` characters so
/// that a huge value costs nothing to display.
pub fn clean(text: &str, max_chars: usize) -> String {
    text.chars()
        .take(max_chars)
        .map(|c| match c {
            '\n' => '↵',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// How many terminal columns `text` occupies.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Shorten `text` to fit `max_width` columns, ending with `…` if cut.
pub fn fit(text: &str, max_width: usize) -> String {
    if width(text) <= max_width {
        return text.to_string();
    }
    let mut fitted = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        // Leave one column for the ellipsis.
        if used + w + 1 > max_width {
            break;
        }
        fitted.push(c);
        used += w;
    }
    if max_width > 0 {
        fitted.push('…');
    }
    fitted
}

/// Make text safe to show across several lines: line breaks are kept,
/// every other control character becomes a space.
pub fn clean_lines(text: &str, max_chars: usize) -> String {
    text.chars()
        .take(max_chars)
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

/// Whether `text` contains `pattern`. As in vim's smartcase: an all-lower
/// pattern ignores case, and any capital letter makes the match exact.
pub fn matches(text: &str, pattern: &str) -> bool {
    if pattern.chars().any(char::is_uppercase) {
        text.contains(pattern)
    } else {
        text.to_lowercase().contains(pattern)
    }
}

/// Standard base64, needed to hand text to the terminal's clipboard.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        // Pack up to three bytes into 24 bits, then emit four 6-bit digits.
        let packed = group.iter().fold(0u32, |acc, &b| (acc << 8) | b as u32) << (8 * (3 - group.len()));
        for digit in 0..4 {
            if digit <= group.len() {
                encoded.push(ALPHABET[(packed >> (18 - 6 * digit)) as usize & 63] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn matching_is_smartcase() {
        assert!(matches("Customer_Orders", "orders"));
        assert!(matches("Customer_Orders", "Orders"));
        assert!(!matches("customer_orders", "Orders"));
    }
}
