//! テスト結果のJSON出力に必要な値とエスケープだけを扱う。
use std::fmt::{self, Write};

pub enum Json {
    Null,
    Bool(bool),
    Number(usize),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}
pub fn object<const N: usize>(fields: [(&'static str, Json); N]) -> Json {
    Json::Object(fields.into())
}
pub fn array(items: impl Iterator<Item = Json>) -> Json {
    Json::Array(items.collect())
}
pub fn string(value: impl ToString) -> Json {
    Json::String(value.to_string())
}
pub fn optional(value: Option<&str>) -> Json {
    value.map(string).unwrap_or(Json::Null)
}

fn quoted(out: &mut fmt::Formatter<'_>, text: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in text.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            c if c <= '\u{1f}' => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}
impl fmt::Display for Json {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => out.write_str("null"),
            Self::Bool(v) => write!(out, "{v}"),
            Self::Number(v) => write!(out, "{v}"),
            Self::String(v) => quoted(out, v),
            Self::Array(items) => {
                out.write_char('[')?;
                for (i, value) in items.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    write!(out, "{value}")?;
                }
                out.write_char(']')
            }
            Self::Object(items) => {
                out.write_char('{')?;
                for (i, (key, value)) in items.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    quoted(out, key)?;
                    write!(out, ":{value}")?;
                }
                out.write_char('}')
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_controls_quotes_and_preserves_unicode() {
        assert_eq!(
            string("\"\\\n\r\t\0\u{8}\u{c}\u{1f}猫👨‍👩").to_string(),
            "\"\\\"\\\\\\n\\r\\t\\u0000\\u0008\\u000c\\u001f猫👨‍👩\""
        );
        for n in 0..32 {
            let s = string(char::from_u32(n).unwrap()).to_string();
            assert!(s.chars().all(|c| c >= ' '));
        }
    }
    #[test]
    fn nested_values_and_empty_containers() {
        assert_eq!(
            object([
                (
                    "x",
                    array(
                        [Json::Null, Json::Bool(false), Json::Number(42), string("a")].into_iter()
                    )
                ),
                ("empty", object([]))
            ])
            .to_string(),
            "{\"x\":[null,false,42,\"a\"],\"empty\":{}}"
        );
        assert_eq!(array(std::iter::empty()).to_string(), "[]");
    }
}
