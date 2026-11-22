//! Steam's non-Steam shortcuts, read as local games.
//!
//! A shortcut's launch options are a line Steam hands to a shell with
//! `%command%` replaced by the program. Ferestre does not run a shell, so the
//! line is split into words the way `sh` would split it -- quotes and
//! backslashes honoured, nothing expanded -- and anything that only a shell
//! could carry out (`;`, `|`, redirections, command substitution) is refused
//! rather than guessed at.

use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// A shortcut's launch options, taken apart.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `NAME=value` words before the first wrapper.
    pub env: BTreeMap<String, String>,
    /// The programs before `%command%` (`gamemoderun`, `mangohud`).
    pub wrapper: Vec<String>,
    /// The game's arguments: words after `%command%`, or every word when
    /// there is no `%command%` (Steam appends them).
    pub args: Vec<String>,
}

const COMMAND: &str = "%command%";

/// Split Steam launch options into environment, wrappers and arguments.
pub fn parse_launch_options(text: &str) -> Result<LaunchOptions> {
    let words = split_words(text)?;
    let mut options = LaunchOptions::default();
    let Some(at) = words.iter().position(|w| w == COMMAND) else {
        options.args = words;
        return Ok(options);
    };
    let mut before = words[..at].iter();
    for word in before.by_ref() {
        match env_assignment(word) {
            Some((name, value)) if options.wrapper.is_empty() => {
                options.env.insert(name.to_owned(), value.to_owned());
            }
            _ => {
                options.wrapper.push(word.clone());
                break;
            }
        }
    }
    options.wrapper.extend(before.cloned());
    options.args = words[at + 1..].to_vec();
    Ok(options)
}

/// `NAME=value`, where `NAME` is `[A-Za-z_][A-Za-z0-9_]*`.
fn env_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut bytes = name.bytes();
    let first = bytes.next()?;
    (first.is_ascii_alphabetic() || first == b'_')
        .then_some(())
        .filter(|_| bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .map(|_| (name, value))
}

/// Split `text` into words like a POSIX shell, expanding nothing. Shell
/// operators outside single quotes are an error naming the operator.
fn split_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(done) = word.take() {
                    words.push(done);
                }
            }
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => bail!("a ' is never closed"),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => bail!("a \" is never closed"),
                        },
                        // A shell would run these even inside double quotes.
                        Some('`') => bail!("the shell operator ` is not supported"),
                        Some('$') if chars.peek() == Some(&'(') => {
                            bail!("the shell operator $( is not supported")
                        }
                        Some(c) => current.push(c),
                        None => bail!("a \" is never closed"),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(c) => word.get_or_insert_with(String::new).push(c),
                None => bail!("the line ends with a lone \\"),
            },
            ';' | '&' | '|' | '<' | '>' | '`' => {
                bail!("the shell operator {c} is not supported")
            }
            '$' if chars.peek() == Some(&'(') => {
                bail!("the shell operator $( is not supported")
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(done) = word {
        words.push(done);
    }
    Ok(words)
}

/// Words written so that [`parse_launch_options`] splits them back the same:
/// bare when made only of `[A-Za-z0-9_./=:,+@%-]`, single-quoted otherwise.
pub fn quote_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let bare = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_./=:,+@%-".contains(c));
            if bare {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `value` with a leading `~/`, `$HOME/` or `${HOME}/` (or the whole value
/// being one of those without the slash) replaced by `home`. Nothing else is
/// expanded.
pub fn expand_home(value: &str, home: &Path) -> String {
    for spelling in ["~", "${HOME}", "$HOME"] {
        if value == spelling {
            return home.display().to_string();
        }
        if let Some(rest) = value
            .strip_prefix(spelling)
            .and_then(|r| r.strip_prefix('/'))
        {
            return home.join(rest).display().to_string();
        }
    }
    value.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn environment_wrappers_and_arguments_split_at_command() {
        let o = parse_launch_options(
            r#"PROTON_ENABLE_WAYLAND=1 DXVK_HDR=1 STEAM_COMPAT_DATA_PATH="/home/me/Games/prefixes/cod mw4" gamemoderun %command% -dx12 "--profile=a b""#,
        )
        .unwrap();
        assert_eq!(
            o.env["STEAM_COMPAT_DATA_PATH"],
            "/home/me/Games/prefixes/cod mw4"
        );
        assert_eq!(o.env.len(), 3);
        assert_eq!(o.wrapper, ["gamemoderun"]);
        assert_eq!(o.args, ["-dx12", "--profile=a b"]);
    }

    #[test]
    fn an_environment_word_after_a_wrapper_stays_in_the_wrapper() {
        let o = parse_launch_options("A=1 env B=2 mangohud %command%").unwrap();
        assert_eq!(o.env.len(), 1);
        assert_eq!(o.wrapper, ["env", "B=2", "mangohud"]);
        assert!(o.args.is_empty());
    }

    #[test]
    fn without_command_every_word_is_an_argument() {
        assert_eq!(
            parse_launch_options("-skipintro 'two words'").unwrap(),
            LaunchOptions {
                args: vec!["-skipintro".into(), "two words".into()],
                ..Default::default()
            }
        );
        assert_eq!(parse_launch_options("").unwrap(), LaunchOptions::default());
    }

    #[test]
    fn shell_operators_are_refused_not_guessed() {
        for text in [
            "A=1 %command% ; rm x",
            "%command% | tee log",
            "X=$(id) %command%",
            "%command% > log",
            "%command% && x",
            "%command% `id`",
            "%command% 'unterminated",
        ] {
            assert!(parse_launch_options(text).is_err(), "{text}");
        }
        let e = parse_launch_options("%command% | tee log").unwrap_err();
        assert!(e.to_string().contains('|'), "{e}");
        assert!(parse_launch_options(r#"%command% "a;b""#).is_ok());
        assert_eq!(
            parse_launch_options(r#"%command% a\;b "x\"y" "\$HOME""#)
                .unwrap()
                .args,
            ["a;b", "x\"y", "$HOME"]
        );
    }

    #[test]
    fn quoted_words_split_back_to_the_same_words() {
        let words: Vec<String> = ["-dx12", "a b", "it's", ""].map(String::from).to_vec();
        assert_eq!(quote_words(&words), r#"-dx12 'a b' 'it'\''s' ''"#);
        assert_eq!(
            parse_launch_options(&quote_words(&words)).unwrap().args,
            words
        );
    }

    #[test]
    fn only_the_home_directory_is_expanded() {
        let home = Path::new("/home/me");
        assert_eq!(expand_home("~/Games/p", home), "/home/me/Games/p");
        assert_eq!(expand_home("${HOME}/p", home), "/home/me/p");
        assert_eq!(expand_home("$HOME/p", home), "/home/me/p");
        assert_eq!(expand_home("~", home), "/home/me");
        assert_eq!(expand_home("$XDG_DATA_HOME/p", home), "$XDG_DATA_HOME/p");
        assert_eq!(expand_home("/a/~/b", home), "/a/~/b");
    }
}
