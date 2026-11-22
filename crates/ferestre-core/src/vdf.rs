//! Text KeyValues: Steam's `config.vdf`, `libraryfolders.vdf` and
//! `compatibilitytool.vdf`.
//!
//! Read into the same [`Value`] tree as binary `shortcuts.vdf`, so one
//! case-insensitive [`Value::get`] serves both. Only reading: Ferestre never
//! writes these files.
//!
//! Tokens are quoted (`"…"`, with `\"`, `\\`, `\n` and `\t` escapes) or bare
//! (a run of anything but whitespace, braces and quotes). `{`/`}` nest, `//`
//! starts a comment to the end of the line, and a `[$CONDITION]` tag is
//! skipped. Duplicate keys are kept in order.

use crate::steam::Value;
use anyhow::{bail, Result};

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokens(text: &str) -> Result<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut line = 1usize;
    while let Some(&c) = chars.peek() {
        match c {
            '\n' => {
                line += 1;
                chars.next();
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '{' => {
                chars.next();
                out.push(Token::Open);
            }
            '}' => {
                chars.next();
                out.push(Token::Close);
            }
            '/' if text_starts_comment(&mut chars) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        line += 1;
                        break;
                    }
                }
            }
            '[' => {
                // A `[$WIN32]` condition: Ferestre reads every branch.
                let start = line;
                chars.next();
                loop {
                    match chars.next() {
                        Some(']') => break,
                        Some('\n') => line += 1,
                        Some(_) => {}
                        None => bail!("line {start}: unterminated [condition]"),
                    }
                }
            }
            '"' => {
                let start = line;
                chars.next();
                let mut value = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('"') => value.push('"'),
                            Some('\\') => value.push('\\'),
                            Some('n') => value.push('\n'),
                            Some('t') => value.push('\t'),
                            Some(other) => {
                                value.push('\\');
                                value.push(other);
                            }
                            None => bail!("line {start}: unterminated string"),
                        },
                        Some(c) => {
                            if c == '\n' {
                                line += 1;
                            }
                            value.push(c);
                        }
                        None => bail!("line {start}: unterminated string"),
                    }
                }
                out.push(Token::Str(value));
            }
            _ => {
                let mut value = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                        break;
                    }
                    value.push(c);
                    chars.next();
                }
                out.push(Token::Str(value));
            }
        }
    }
    Ok(out)
}

/// Whether the `/` at the front of `chars` starts a `//` comment.
fn text_starts_comment(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut ahead = chars.clone();
    ahead.next();
    ahead.next() == Some('/')
}

/// Parse a text KeyValues document into a map of its top-level pairs.
pub fn parse(text: &str) -> Result<Value> {
    let tokens = tokens(text)?;
    let mut at = 0;
    let entries = pairs(&tokens, &mut at, false)?;
    Ok(Value::Map(entries))
}

fn pairs(tokens: &[Token], at: &mut usize, nested: bool) -> Result<Vec<(String, Value)>> {
    let mut entries = Vec::new();
    loop {
        match tokens.get(*at) {
            None if nested => bail!("a {{ is never closed"),
            None => return Ok(entries),
            Some(Token::Close) if nested => {
                *at += 1;
                return Ok(entries);
            }
            Some(Token::Close) => bail!("a }} closes nothing"),
            Some(Token::Open) => bail!("a {{ where a key belongs"),
            Some(Token::Str(key)) => {
                *at += 1;
                let value = match tokens.get(*at) {
                    Some(Token::Str(value)) => {
                        *at += 1;
                        Value::Str(value.clone())
                    }
                    Some(Token::Open) => {
                        *at += 1;
                        Value::Map(pairs(tokens, at, true)?)
                    }
                    Some(Token::Close) | None => bail!("{key:?} has no value"),
                };
                entries.push((key.clone(), value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_vdf_compat_tool_mapping_reads() {
        let text = r#"
"InstallConfigStore" { "Software" { "Valve" { "Steam" { "CompatToolMapping" {
  "3000000001" { "name" "GE-Proton10-25" "config" "" "priority" "250" } } } } } }
"#;
        let doc = parse(text).unwrap();
        let name = doc
            .get("InstallConfigStore")
            .and_then(|v| v.get("software"))
            .and_then(|v| v.get("Valve"))
            .and_then(|v| v.get("Steam"))
            .and_then(|v| v.get("CompatToolMapping"))
            .and_then(|v| v.get("3000000001"))
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str());
        assert_eq!(name, Some("GE-Proton10-25"));
    }

    #[test]
    fn comments_escapes_and_conditions() {
        let doc = parse(r#""k" "a\"b""#).unwrap();
        assert_eq!(doc.get("k").and_then(|v| v.as_str()), Some("a\"b"));
        let doc = parse("\"k\" \"v\" [$WIN32]\n\"j\" \"w\"").unwrap();
        assert_eq!(doc.get("k").and_then(|v| v.as_str()), Some("v"));
        assert_eq!(doc.get("j").and_then(|v| v.as_str()), Some("w"));
        let doc = parse("// x\n\"k\" { bare value // trailing\n }").unwrap();
        assert_eq!(
            doc.get("k")
                .and_then(|v| v.get("bare"))
                .and_then(|v| v.as_str()),
            Some("value")
        );
        let doc = parse(r#""p" "C:\\Games\tx" "d" "1" "d" "2""#).unwrap();
        assert_eq!(doc.get("p").and_then(|v| v.as_str()), Some("C:\\Games\tx"));
        // Duplicate keys are kept, in order.
        let dups: Vec<_> = doc
            .as_map()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "d")
            .map(|(_, v)| v.as_str().unwrap())
            .collect();
        assert_eq!(dups, ["1", "2"]);
    }

    #[test]
    fn unbalanced_text_is_an_error() {
        for text in [r#""a" {"#, r#""a" "b"#, r#""a" "b" }"#, r#""a""#] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
