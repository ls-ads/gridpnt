//! How Databricks result values become gridpnt values.
//!
//! In the JSON_ARRAY result format every value arrives as text, or null,
//! whatever its type. The column's declared type says how to read it:
//! numbers become numbers, timestamps are tidied, binary is shown as hex,
//! and arrays, maps and structs are rewritten by `nested`.

use std::fmt::Write;

use super::nested;
use crate::backend::Value;

/// How the values of one result column are read, worked out once per
/// result from the column's declared type.
pub enum ColumnKind {
    Integer,
    Real,
    Decimal,
    Timestamp,
    Binary,
    /// An array, map or struct whose declared type gridpnt could read.
    Nested(nested::Type),
    Text,
}

impl ColumnKind {
    /// `type_name` is Databricks' short name for the type, such as `INT` or
    /// `STRUCT`; `type_text` is the whole declaration, such as
    /// `STRUCT<id: INT, name: STRING>`.
    pub fn of(type_name: &str, type_text: &str) -> ColumnKind {
        match type_name {
            "BYTE" | "SHORT" | "INT" | "LONG" => ColumnKind::Integer,
            "FLOAT" | "DOUBLE" => ColumnKind::Real,
            "DECIMAL" => ColumnKind::Decimal,
            "TIMESTAMP" | "TIMESTAMP_NTZ" => ColumnKind::Timestamp,
            "BINARY" => ColumnKind::Binary,
            // A declaration gridpnt cannot read leaves the values as sent.
            "ARRAY" | "MAP" | "STRUCT" => nested::parse_type(type_text).map_or(ColumnKind::Text, ColumnKind::Nested),
            _ => ColumnKind::Text,
        }
    }

    /// Read one cell. Text that does not fit the type is shown as sent.
    pub fn value(&self, text: Option<String>) -> Value {
        let Some(text) = text else { return Value::Null };
        match self {
            ColumnKind::Integer => text.parse().map(Value::Integer).unwrap_or(Value::Text(text)),
            ColumnKind::Real => text.parse().map(Value::Real).unwrap_or(Value::Text(text)),
            ColumnKind::Decimal => Value::Decimal(text),
            ColumnKind::Timestamp => Value::Text(tidy_timestamp(&text)),
            ColumnKind::Binary => Value::Text(binary_as_hex(&text).unwrap_or(text)),
            ColumnKind::Nested(declared) => Value::Text(nested::tidy(&text, declared).unwrap_or(text)),
            ColumnKind::Text => Value::Text(text),
        }
    }
}

/// Databricks sends `2016-02-13T21:47:53.000Z`; show `2016-02-13 21:47:53`.
/// The `Z` only says the time is in UTC, which it always is here, and a
/// fraction of all zeros says nothing. Anything unexpected is left alone.
pub fn tidy_timestamp(text: &str) -> String {
    let Some((date, time)) = text.split_once('T') else {
        return text.to_string();
    };
    let time = time.strip_suffix('Z').unwrap_or(time);
    let time = match time.split_once('.') {
        Some((seconds, fraction)) if fraction.bytes().all(|digit| digit == b'0') => seconds,
        _ => time,
    };
    format!("{date} {time}")
}

/// Databricks sends binary values as base64, which reads like text. Show
/// them as a SQL hex literal instead, such as `X'DEADBEEF'`: plainly bytes,
/// and ready to paste back into a query. `None` if `text` is not base64.
pub fn binary_as_hex(text: &str) -> Option<String> {
    let mut hex = String::with_capacity(text.len() * 3 / 2 + 3);
    hex.push_str("X'");
    // Each base64 digit carries 6 bits; every 8 bits collected make a byte.
    let (mut bits, mut held) = (0u32, 0u32);
    for digit in text.trim_end_matches('=').bytes() {
        let value = match digit {
            b'A'..=b'Z' => digit - b'A',
            b'a'..=b'z' => digit - b'a' + 26,
            b'0'..=b'9' => digit - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = ((bits << 6) | u32::from(value)) & 0xFFFF;
        held += 6;
        if held >= 8 {
            held -= 8;
            // Writing to a String cannot fail.
            let _ = write!(hex, "{:02X}", (bits >> held) & 0xFF);
        }
    }
    hex.push('\'');
    Some(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_tidied() {
        assert_eq!(tidy_timestamp("2016-02-13T21:47:53.000Z"), "2016-02-13 21:47:53");
        assert_eq!(tidy_timestamp("2016-02-13T21:47:53.120Z"), "2016-02-13 21:47:53.120");
        assert_eq!(tidy_timestamp("2016-02-13T21:47:53.000"), "2016-02-13 21:47:53");
        assert_eq!(tidy_timestamp("not a timestamp"), "not a timestamp");
    }

    #[test]
    fn binary_is_shown_as_hex() {
        assert_eq!(binary_as_hex("3q2+7w==").as_deref(), Some("X'DEADBEEF'"));
        assert_eq!(binary_as_hex("AA==").as_deref(), Some("X'00'"));
        assert_eq!(binary_as_hex("").as_deref(), Some("X''"));
        assert_eq!(binary_as_hex("not base64!"), None);
    }

    #[test]
    fn text_that_does_not_fit_its_type_is_shown_as_sent() {
        let as_sent = |sent: &str| Value::Text(sent.into());
        assert_eq!(
            ColumnKind::of("INT", "INT").value(Some("12".into())),
            Value::Integer(12)
        );
        assert_eq!(
            ColumnKind::of("INT", "INT").value(Some("twelve".into())),
            as_sent("twelve")
        );
        assert_eq!(
            ColumnKind::of("BINARY", "BINARY").value(Some("%%".into())),
            as_sent("%%")
        );
        assert_eq!(
            ColumnKind::of("ARRAY", "ARRAY<").value(Some(r#"["1"]"#.into())),
            as_sent(r#"["1"]"#)
        );
        assert_eq!(ColumnKind::of("ARRAY", "ARRAY<INT>").value(None), Value::Null);
    }
}
