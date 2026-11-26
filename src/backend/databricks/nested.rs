//! Arrays, maps and structs in Databricks results.
//!
//! Databricks sends them as JSON in which every number is a quoted string
//! and struct fields come in no particular order: an `ARRAY<INT>` holding
//! 1, 2 and null arrives as `["1","2",null]`. Each column also comes with
//! its declared type, such as `STRUCT<id: INT NOT NULL, tags: ARRAY<STRING>>`.
//! Reading that declaration lets gridpnt show a value as it was meant:
//! numbers and booleans without quotes, struct fields in declared order,
//! timestamps tidied, binary as hex and a VARIANT as its JSON, as in
//! columns of those types.
//!
//! What is shown is for reading, not strict JSON: `NaN` and `X'…'` appear
//! without quotes. A value that does not match its declaration, or a
//! declaration gridpnt cannot read, is shown exactly as sent instead.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};

use super::values::{binary_as_hex, tidy_timestamp};

/// A declared type, reduced to what showing a value needs.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// Numbers and booleans: shown without quotes.
    Bare,
    /// Shown as hex, as in a binary column.
    Binary,
    /// Tidied, as in a timestamp column.
    Timestamp,
    /// Strings, dates and everything else: shown as quoted text.
    Text,
    /// JSON already, sent as a string: shown as the JSON it holds, as in a
    /// VARIANT column.
    Variant,
    Array(Box<Type>),
    /// The type of the map's values. Keys are always shown as quoted text,
    /// as JSON requires.
    Map(Box<Type>),
    /// The fields in declared order.
    Struct(Vec<(String, Type)>),
}

/// Read a declaration such as `MAP<STRING, ARRAY<INT>>`. `None` if it is
/// not understood.
pub fn parse_type(declared: &str) -> Option<Type> {
    let mut reader = Reader(declared);
    let parsed = reader.read_type()?;
    reader.0.trim().is_empty().then_some(parsed)
}

/// Show `sent`, a value as Databricks sent it, the way its declared type
/// says. `None` if the value does not match the declaration.
pub fn tidy(sent: &str, declared: &Type) -> Option<String> {
    let json: Json = serde_json::from_str(sent).ok()?;
    let mut shown = String::with_capacity(sent.len());
    show(&json, declared, &mut shown)?;
    Some(shown)
}

/// Append `json` to `out` the way `declared` says. `None` on a mismatch.
fn show(json: &Json, declared: &Type, out: &mut String) -> Option<()> {
    match (declared, json) {
        (_, Json::Null) => out.push_str("null"),
        (Type::Array(element), Json::List(items)) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                show(item, element, out)?;
            }
            out.push(']');
        }
        (Type::Map(value_type), Json::Object(entries)) => {
            out.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                quote(key, out);
                out.push(':');
                show(value, value_type, out)?;
            }
            out.push('}');
        }
        (Type::Struct(fields), Json::Object(entries)) => {
            // Each declared field takes the entry with its name. A field
            // missing, or an entry left over, means the value is not what
            // was declared.
            if fields.len() != entries.len() {
                return None;
            }
            let mut taken = vec![false; entries.len()];
            out.push('{');
            for (index, (name, field_type)) in fields.iter().enumerate() {
                let at = (0..entries.len()).find(|&at| !taken[at] && entries[at].0 == *name)?;
                taken[at] = true;
                if index > 0 {
                    out.push(',');
                }
                quote(name, out);
                out.push(':');
                show(&entries[at].1, field_type, out)?;
            }
            out.push('}');
        }
        (Type::Array(_) | Type::Map(_) | Type::Struct(_), _) => return None,
        (_, Json::Bool(yes)) => out.push_str(if *yes { "true" } else { "false" }),
        (Type::Bare, Json::Text(text)) if looks_bare(text) => out.push_str(text),
        (Type::Timestamp, Json::Text(text)) => quote(&tidy_timestamp(text), out),
        (Type::Binary, Json::Text(text)) => match binary_as_hex(text) {
            Some(hex) => out.push_str(&hex),
            None => quote(text, out),
        },
        (Type::Variant, Json::Text(json)) => out.push_str(json),
        (_, Json::Text(text)) => quote(text, out),
        // JSON where text was expected: not understood, so the whole
        // value is shown as sent.
        (_, Json::List(_) | Json::Object(_)) => return None,
    }
    Some(())
}

/// Whether text sent for a number or boolean really is one, and so can be
/// shown without quotes.
fn looks_bare(text: &str) -> bool {
    let numeric = |byte: u8| byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E');
    matches!(text, "true" | "false" | "NaN" | "Infinity" | "-Infinity")
        || (text.bytes().any(|byte| byte.is_ascii_digit()) && text.bytes().all(numeric))
}

/// Append `text` as a quoted, escaped JSON string.
fn quote(text: &str, out: &mut String) {
    // Turning a string into JSON cannot fail.
    out.push_str(&serde_json::to_string(text).unwrap_or_default());
}

/// What is left of a declaration while it is being read.
struct Reader<'a>(&'a str);

impl<'a> Reader<'a> {
    fn read_type(&mut self) -> Option<Type> {
        let parsed = match self.word().to_ascii_uppercase().as_str() {
            "ARRAY" => {
                self.expect('<')?;
                let element = self.read_type()?;
                self.expect('>')?;
                Type::Array(Box::new(element))
            }
            "MAP" => {
                self.expect('<')?;
                self.read_type()?; // the keys' type: keys are always shown quoted
                self.expect(',')?;
                let value = self.read_type()?;
                self.expect('>')?;
                Type::Map(Box::new(value))
            }
            "STRUCT" => {
                self.expect('<')?;
                let mut fields = Vec::new();
                while !self.next_is('>') {
                    if !fields.is_empty() {
                        self.expect(',')?;
                    }
                    let name = self.field_name()?;
                    self.expect(':')?;
                    fields.push((name, self.read_type()?));
                }
                self.expect('>')?;
                Type::Struct(fields)
            }
            "BOOLEAN" | "TINYINT" | "BYTE" | "SMALLINT" | "SHORT" | "INT" | "INTEGER" | "BIGINT" | "LONG" | "FLOAT"
            | "REAL" | "DOUBLE" | "DECIMAL" | "DEC" | "NUMERIC" => Type::Bare,
            "TIMESTAMP" | "TIMESTAMP_NTZ" | "TIMESTAMP_LTZ" => Type::Timestamp,
            "BINARY" => Type::Binary,
            "VARIANT" => Type::Variant,
            "" => return None,
            _ => Type::Text,
        };
        // Step over what may follow the type's name: `(10,2)`, `NOT NULL`,
        // `COMMENT '…'`, or the rest of `INTERVAL DAY TO SECOND`.
        self.skip_to_separator();
        Some(parsed)
    }

    /// The next word, such as a type's name; empty if there is none.
    fn word(&mut self) -> &'a str {
        self.0 = self.0.trim_start();
        let end = self
            .0
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(self.0.len());
        let (word, rest) = self.0.split_at(end);
        self.0 = rest;
        word
    }

    /// Step over `expected`, after any spaces. `None` if it is not next.
    fn expect(&mut self, expected: char) -> Option<()> {
        self.0 = self.0.trim_start().strip_prefix(expected)?;
        Some(())
    }

    /// Whether `expected` comes next, after any spaces.
    fn next_is(&self, expected: char) -> bool {
        self.0.trim_start().starts_with(expected)
    }

    /// A struct field's name: plain, or in backquotes when it holds unusual
    /// characters, with a doubled backquote standing for one.
    fn field_name(&mut self) -> Option<String> {
        self.0 = self.0.trim_start();
        if let Some(quoted) = self.0.strip_prefix('`') {
            let mut name = String::new();
            let mut chars = quoted.char_indices();
            while let Some((at, c)) = chars.next() {
                if c != '`' {
                    name.push(c);
                } else if quoted[at + 1..].starts_with('`') {
                    name.push('`');
                    chars.next();
                } else {
                    self.0 = &quoted[at + 1..];
                    return Some(name);
                }
            }
            return None; // the closing backquote is missing
        }
        let end = self.0.find([':', ',', '<', '>']).unwrap_or(self.0.len());
        let name = self.0[..end].trim();
        self.0 = &self.0[end..];
        (!name.is_empty()).then(|| name.to_string())
    }

    /// Move past anything up to the next `,` or `>` at this level, stepping
    /// over brackets and quoted text, such as `(10,2)` or `COMMENT 'a, b'`.
    fn skip_to_separator(&mut self) {
        let mut depth = 0;
        let mut chars = self.0.char_indices();
        while let Some((at, c)) = chars.next() {
            match c {
                ',' | '>' if depth == 0 => {
                    self.0 = &self.0[at..];
                    return;
                }
                '(' => depth += 1,
                ')' => depth -= 1,
                '\'' | '"' | '`' => {
                    // On to the closing quote; a backslash escapes the next character.
                    while let Some((_, inner)) = chars.next() {
                        if inner == '\\' {
                            chars.next();
                        } else if inner == c {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        self.0 = "";
    }
}

/// JSON as Databricks sends it inside a value, with each object's entries
/// kept in the order they came. Databricks quotes every number, so a bare
/// number makes reading fail and the value is shown as sent: no number is
/// ever re-read, and perhaps rounded, on its way to the screen.
enum Json {
    Null,
    Bool(bool),
    Text(String),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

/// Builds a `Json` from what serde_json reads. It has no methods for
/// numbers, and serde's default for those is an error.
struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with quoted numbers")
    }

    fn visit_unit<E: Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::Text(value.to_string()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Json, A::Error> {
        let mut list = Vec::new();
        while let Some(item) = items.next_element()? {
            list.push(item);
        }
        Ok(Json::List(list))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Json, A::Error> {
        let mut object = Vec::new();
        while let Some(entry) = entries.next_entry()? {
            object.push(entry);
        }
        Ok(Json::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tidy_as(declared: &str, sent: &str) -> Option<String> {
        tidy(sent, &parse_type(declared)?)
    }

    #[test]
    fn numbers_and_booleans_lose_their_quotes() {
        assert_eq!(
            tidy_as("ARRAY<INT>", r#"["1","2",null]"#).as_deref(),
            Some("[1,2,null]")
        );
        assert_eq!(
            tidy_as("ARRAY<ARRAY<BIGINT>>", r#"[["1"],["2","3"]]"#).as_deref(),
            Some("[[1],[2,3]]")
        );
        assert_eq!(
            tidy_as("MAP<STRING, INT>", r#"{"k":"1","j":"2"}"#).as_deref(),
            Some(r#"{"k":1,"j":2}"#)
        );
        assert_eq!(
            tidy_as("ARRAY<DOUBLE>", r#"["1.5","NaN","-Infinity","1.0E300"]"#).as_deref(),
            Some("[1.5,NaN,-Infinity,1.0E300]")
        );
        assert_eq!(
            tidy_as("ARRAY<BOOLEAN>", r#"["true",false]"#).as_deref(),
            Some("[true,false]")
        );
        // Text stays quoted, even text that looks like a number.
        assert_eq!(
            tidy_as("ARRAY<STRING>", r#"["1","say \"hi\""]"#).as_deref(),
            Some(r#"["1","say \"hi\""]"#)
        );
        assert_eq!(
            tidy_as("ARRAY<INTERVAL DAY TO SECOND>", r#"["INTERVAL '1' DAY"]"#).as_deref(),
            Some(r#"["INTERVAL '1' DAY"]"#)
        );
    }

    #[test]
    fn struct_fields_come_in_declared_order() {
        let declared = "STRUCT<id: INT NOT NULL, name: STRING COMMENT 'who, or <what>', \
                        tags: ARRAY<STRING> NOT NULL, at: TIMESTAMP, price: DECIMAL(4,2), raw: BINARY>";
        let sent = r#"{"name":"x y","tags":["a","b"],"price":"9.50","at":"2024-01-01T00:00:00.000Z","raw":"3q2+7w==","id":"7"}"#;
        assert_eq!(
            tidy_as(declared, sent).as_deref(),
            Some(r#"{"id":7,"name":"x y","tags":["a","b"],"at":"2024-01-01 00:00:00","price":9.50,"raw":X'DEADBEEF'}"#)
        );
        // Names with unusual characters are quoted in the declaration.
        assert_eq!(
            tidy_as("STRUCT<`a b`: INT, `c``d`: STRING>", r#"{"c`d":"x","a b":"1"}"#).as_deref(),
            Some(r#"{"a b":1,"c`d":"x"}"#)
        );
        assert_eq!(
            tidy_as(
                "ARRAY<STRUCT<a: DOUBLE NOT NULL, b: STRING>>",
                r#"[{"a":"1.5","b":null}]"#
            )
            .as_deref(),
            Some(r#"[{"a":1.5,"b":null}]"#)
        );
    }

    #[test]
    fn a_variant_inside_is_shown_as_its_json() {
        // Databricks sends a VARIANT inside a struct as a string of JSON.
        assert_eq!(
            tidy_as("STRUCT<v: VARIANT, n: INT>", r#"{"v":"{\"a\":1}","n":"2"}"#).as_deref(),
            Some(r#"{"v":{"a":1},"n":2}"#)
        );
    }

    #[test]
    fn anything_unexpected_is_left_as_sent() {
        assert_eq!(parse_type("ARRAY<INT"), None);
        assert_eq!(parse_type("STRUCT<a INT>"), None);
        assert_eq!(parse_type("MAP<STRING>"), None);
        assert_eq!(tidy_as("ARRAY<INT>", r#"{"a":"1"}"#), None); // not an array
        assert_eq!(tidy_as("STRUCT<a: INT>", r#"{"a":"1","b":"2"}"#), None); // an undeclared field
        assert_eq!(tidy_as("STRUCT<a: INT, b: INT>", r#"{"a":"1"}"#), None); // a missing field
        assert_eq!(tidy_as("ARRAY<BIGINT>", "[9007199254740993]"), None); // a bare number
        assert_eq!(tidy_as("STRUCT<v: VARIANT>", r#"{"v":{"a":"x"}}"#), None); // a variant sent as JSON, not as text
        assert_eq!(tidy_as("ARRAY<INT>", "not json"), None);
    }
}
