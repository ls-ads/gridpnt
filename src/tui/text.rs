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
