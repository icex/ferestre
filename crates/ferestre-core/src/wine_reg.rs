//! One program's DLL overrides in a Wine prefix's `user.reg`.
//!
//! Wine reads `HKCU\Software\Wine\AppDefaults\<program>\DllOverrides` for a
//! program named `<program>` (its file name, any folder), after
//! `WINEDLLOVERRIDES` and before the prefix-wide `DllOverrides`. A game that
//! Steam starts gets no environment from Ferestre, so this is where neural
//! rendering's `dxgi=native,builtin` goes: in that game's own prefix, for that
//! game's program alone.
//!
//! The file is edited as text, a line at a time; every other line stays as it
//! was. Wine must not be running in the prefix while it is written, or
//! wineserver writes its own copy back over it when it exits.

use anyhow::{bail, Result};

/// A program name Wine would look up, refused when it is not one: plain ASCII
/// with no folder, quote or bracket.
pub fn check_program(program: &str) -> Result<()> {
    let fine = !program.is_empty()
        && program
            .bytes()
            .all(|b| (0x20..0x7f).contains(&b) && !b"\\/\"[]".contains(&b));
    if !fine {
        bail!("{program:?}: Wine cannot be given DLL overrides for this program name");
    }
    Ok(())
}

/// The section of `program`'s overrides, as `user.reg` writes it.
fn section(program: &str) -> String {
    format!("Software\\\\Wine\\\\AppDefaults\\\\{program}\\\\DllOverrides")
}

/// The key of a `[key] stamp` header line, if `line` is one.
fn header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    rest.find(']').map(|end| &rest[..end])
}

/// The name and value of a `"name"="value"` line.
fn string_value(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('"')?;
    let (name, rest) = rest.split_once("\"=\"")?;
    Some((name, rest.strip_suffix('"')?))
}

/// The line range of `program`'s section, header excluded.
fn find_section(lines: &[&str], program: &str) -> Option<(usize, usize)> {
    let wanted = section(program);
    let start = lines
        .iter()
        .position(|l| header(l).is_some_and(|k| k.eq_ignore_ascii_case(&wanted)))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| header(l).is_some())
        .map_or(lines.len(), |i| start + 1 + i);
    Some((start + 1, end))
}

/// What `program`'s override for `dll` is in `text`, if it has one.
pub fn get(text: &str, program: &str, dll: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = find_section(&lines, program)?;
    lines[start..end].iter().find_map(|l| {
        string_value(l)
            .filter(|(name, _)| name.eq_ignore_ascii_case(dll))
            .map(|(_, value)| value.to_owned())
    })
}

/// `text` with `program`'s override for `dll` set to `value`, or removed
/// when `value` is `None`. A section that is not there is added at the end.
pub fn set(text: &str, program: &str, dll: &str, value: Option<&str>, now: u64) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let line = value.map(|v| format!("\"{dll}\"=\"{v}\""));
    match find_section(&borrowed, program) {
        Some((start, end)) => {
            let at = borrowed[start..end].iter().position(|l| {
                string_value(l).is_some_and(|(name, _)| name.eq_ignore_ascii_case(dll))
            });
            // After the header's own `#time=` and other `#` lines.
            let first_value = borrowed[start..end]
                .iter()
                .position(|l| !l.starts_with('#'))
                .map_or(end, |i| start + i);
            match (at, line) {
                (Some(i), Some(line)) => lines[start + i] = line,
                (Some(i), None) => {
                    lines.remove(start + i);
                }
                (None, Some(line)) => lines.insert(first_value, line),
                (None, None) => {}
            }
        }
        None => {
            if let Some(line) = line {
                if lines.last().is_some_and(|l| !l.is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{}] {now}", section(program)));
                lines.push(line);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\n\n#arch=win64\n\n\
[Software\\\\Wine\\\\AppDefaults\\\\Avengers.exe\\\\DllOverrides] 1774238072\n#time=1dcba78c2140f5e\n\
\"atiadlxx\"=\"builtin\"\n\n[Software\\\\Wine\\\\Drivers] 1774238072\n\"Graphics\"=\"x11\"\n";

    #[test]
    fn an_override_goes_into_its_programs_section_and_comes_out_again() {
        let added = set(REG, "bf6.exe", "dxgi", Some("native,builtin"), 7);
        assert!(added.starts_with(REG), "{added}");
        assert!(added.ends_with(
            "\n[Software\\\\Wine\\\\AppDefaults\\\\bf6.exe\\\\DllOverrides] 7\n\"dxgi\"=\"native,builtin\"\n"
        ), "{added}");
        assert_eq!(
            get(&added, "BF6.EXE", "DXGI").as_deref(),
            Some("native,builtin")
        );
        assert_eq!(get(&added, "Avengers.exe", "dxgi"), None);

        // A second value lands in the same section, after its #time line.
        let both = set(REG, "Avengers.exe", "winmm", Some("native,builtin"), 7);
        assert!(
            both.contains(
                "#time=1dcba78c2140f5e\n\"winmm\"=\"native,builtin\"\n\"atiadlxx\"=\"builtin\"\n"
            ),
            "{both}"
        );
        // Replaced in place, then removed, leaving every other line as it was.
        let changed = set(&both, "avengers.exe", "atiadlxx", Some("native"), 9);
        assert_eq!(
            get(&changed, "Avengers.exe", "atiadlxx").as_deref(),
            Some("native")
        );
        let gone = set(
            &set(&changed, "Avengers.exe", "winmm", None, 9),
            "Avengers.exe",
            "atiadlxx",
            Some("builtin"),
            9,
        );
        assert_eq!(gone, REG);
        // Removing what is not there changes nothing.
        assert_eq!(set(REG, "x.exe", "dxgi", None, 1), REG);
    }

    #[test]
    fn a_program_name_wine_would_not_look_up_is_refused() {
        check_program("bf6.exe").unwrap();
        check_program("Cyberpunk 2077.exe").unwrap();
        for bad in ["", "a\\b.exe", "a/b.exe", "a\"b.exe", "a]b.exe", "é.exe"] {
            assert!(check_program(bad).is_err(), "{bad}");
        }
    }
}
