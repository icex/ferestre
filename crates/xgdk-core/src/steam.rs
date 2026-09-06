//! Non-Steam shortcuts: reading and writing `shortcuts.vdf`.
//!
//! People add these titles to Steam for the overlay, controller configuration
//! and Remote Play, and doing it by hand means typing a launch command into a
//! dialog and getting it subtly wrong. The launcher already knows the command,
//! so it can write the entry.
//!
//! `shortcuts.vdf` is binary KeyValues, which is a simple format but an
//! unforgiving one: it is Steam's own file, it holds every shortcut a person
//! has made, and a launcher that writes it carelessly destroys work that has
//! nothing to do with it. So this parses the whole file, keeps every field it
//! did not put there -- including ones it has never heard of -- and writes it
//! back whole.
//!
//! One thing no amount of care fixes: **Steam rewrites this file from memory
//! when it exits**. Editing it while Steam is running loses the edit. Callers
//! have to say so; [`is_running`] is here to let them.

use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Binary KeyValues node types.
const TYPE_MAP: u8 = 0x00;
const TYPE_STRING: u8 = 0x01;
const TYPE_INT32: u8 = 0x02;
const END_MAP: u8 = 0x08;

/// A value in a binary KeyValues document.
///
/// Only the three types `shortcuts.vdf` uses. An unknown type is an error
/// rather than something to skip, because a file we do not fully understand is
/// one we must not rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Ordered, because Steam's entries are keyed "0", "1", "2" and the order
    /// they are written in is the order they appear in the library.
    Map(Vec<(String, Value)>),
    Str(String),
    Int(i32),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// Case-insensitive lookup: Steam has spelled these keys differently across
    /// versions ("appid" and "AppId" both occur in the wild).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self
            .bytes
            .get(self.at)
            .ok_or_else(|| anyhow!("truncated at byte {}", self.at))?;
        self.at += 1;
        Ok(b)
    }

    fn cstring(&mut self) -> Result<String> {
        let start = self.at;
        let end = self.bytes[start..]
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| anyhow!("unterminated string at byte {start}"))?
            + start;
        self.at = end + 1;
        // Steam writes UTF-8 but a shortcut name can come from a filesystem
        // that is not. Lossy, because refusing to show someone their library
        // over one bad byte in one name is the wrong trade.
        Ok(String::from_utf8_lossy(&self.bytes[start..end]).into_owned())
    }

    fn int32(&mut self) -> Result<i32> {
        let end = self.at + 4;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| anyhow!("truncated int at byte {}", self.at))?;
        self.at = end;
        Ok(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn map(&mut self) -> Result<Value> {
        let mut entries = Vec::new();
        loop {
            let kind = self.byte()?;
            if kind == END_MAP {
                return Ok(Value::Map(entries));
            }
            let key = self.cstring()?;
            let value = match kind {
                TYPE_MAP => self.map()?,
                TYPE_STRING => Value::Str(self.cstring()?),
                TYPE_INT32 => Value::Int(self.int32()?),
                other => bail!("unknown value type {other:#04x} for key {key:?}"),
            };
            entries.push((key, value));
        }
    }
}

/// Parse a binary KeyValues document.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    let mut reader = Reader { bytes, at: 0 };
    let kind = reader.byte()?;
    if kind != TYPE_MAP {
        bail!("not a binary KeyValues document (first byte {kind:#04x})");
    }
    let name = reader.cstring()?;
    let body = reader.map()?;
    Ok(Value::Map(vec![(name, body)]))
}

fn write_value(out: &mut Vec<u8>, key: &str, value: &Value) {
    match value {
        Value::Map(entries) => {
            out.push(TYPE_MAP);
            out.extend_from_slice(key.as_bytes());
            out.push(0);
            for (k, v) in entries {
                write_value(out, k, v);
            }
            out.push(END_MAP);
        }
        Value::Str(s) => {
            out.push(TYPE_STRING);
            out.extend_from_slice(key.as_bytes());
            out.push(0);
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        Value::Int(i) => {
            out.push(TYPE_INT32);
            out.extend_from_slice(key.as_bytes());
            out.push(0);
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
}

/// Serialise a document parsed by [`parse`].
pub fn serialise(document: &Value) -> Result<Vec<u8>> {
    let entries = document
        .as_map()
        .ok_or_else(|| anyhow!("a document is a map"))?;
    let mut out = Vec::new();
    for (key, value) in entries {
        write_value(&mut out, key, value);
    }
    Ok(out)
}

/// CRC-32, the ordinary one, computed without a table or a dependency.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The id Steam gives a non-Steam shortcut.
///
/// Not a choice: Steam derives it from the executable and the name, so anything
/// that wants to name the shortcut afterwards -- artwork files, a `steam://`
/// URL -- has to derive the same number the same way.
pub fn shortcut_app_id(exe: &str, app_name: &str) -> u32 {
    let mut key = Vec::new();
    key.extend_from_slice(exe.as_bytes());
    key.extend_from_slice(app_name.as_bytes());
    crc32(&key) | 0x8000_0000
}

/// A shortcut, in the terms the launcher cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub app_name: String,
    /// Quoted, as Steam stores it. [`Shortcut::new`] does the quoting.
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
    pub icon: String,
}

impl Shortcut {
    /// Steam stores `Exe` and `StartDir` quoted, and a path with a space in it
    /// silently fails to launch when they are not.
    pub fn new(
        app_name: impl Into<String>,
        exe: &Path,
        start_dir: &Path,
        launch_options: impl Into<String>,
    ) -> Self {
        Shortcut {
            app_name: app_name.into(),
            exe: format!("\"{}\"", exe.display()),
            start_dir: format!("\"{}\"", start_dir.display()),
            launch_options: launch_options.into(),
            icon: String::new(),
        }
    }

    pub fn app_id(&self) -> u32 {
        shortcut_app_id(&self.exe, &self.app_name)
    }

    fn to_value(&self) -> Value {
        Value::Map(vec![
            ("appid".into(), Value::Int(self.app_id() as i32)),
            ("AppName".into(), Value::Str(self.app_name.clone())),
            ("Exe".into(), Value::Str(self.exe.clone())),
            ("StartDir".into(), Value::Str(self.start_dir.clone())),
            ("icon".into(), Value::Str(self.icon.clone())),
            ("ShortcutPath".into(), Value::Str(String::new())),
            (
                "LaunchOptions".into(),
                Value::Str(self.launch_options.clone()),
            ),
            ("IsHidden".into(), Value::Int(0)),
            ("AllowDesktopConfig".into(), Value::Int(1)),
            ("AllowOverlay".into(), Value::Int(1)),
            ("OpenVR".into(), Value::Int(0)),
            ("Devkit".into(), Value::Int(0)),
            ("DevkitGameID".into(), Value::Str(String::new())),
            ("DevkitOverrideAppID".into(), Value::Int(0)),
            ("LastPlayTime".into(), Value::Int(0)),
            ("FlatpakAppID".into(), Value::Str(String::new())),
            ("tags".into(), Value::Map(vec![])),
        ])
    }
}

/// Add or replace a shortcut in a parsed `shortcuts.vdf`, keeping everything
/// else exactly as it was.
///
/// Matching is by `AppName`, not by app id: the id is derived from the exe, so
/// a launcher that changed where it installs would add a second entry for the
/// same game rather than updating the first. Returns whether an existing entry
/// was replaced.
pub fn upsert(document: &mut Value, shortcut: &Shortcut) -> Result<bool> {
    let shortcuts = document
        .as_map()
        .and_then(|entries| {
            entries
                .iter()
                .position(|(k, _)| k.eq_ignore_ascii_case("shortcuts"))
        })
        .ok_or_else(|| anyhow!("no \"shortcuts\" key: not a shortcuts.vdf"))?;
    let Value::Map(root) = document else {
        unreachable!("checked above");
    };
    let Value::Map(entries) = &mut root[shortcuts].1 else {
        bail!("\"shortcuts\" is not a map");
    };

    let existing = entries.iter().position(|(_, v)| {
        v.get("AppName")
            .and_then(Value::as_str)
            .is_some_and(|name| name == shortcut.app_name)
    });

    match existing {
        Some(at) => {
            // Replace only the fields we own. A person may have set tags, or
            // turned the overlay off; that is theirs, not ours to reset.
            let key = entries[at].0.clone();
            let mut merged = entries[at].1.clone();
            if let Value::Map(fields) = &mut merged {
                let ours = shortcut.to_value();
                for (name, value) in ours.as_map().unwrap_or(&[]) {
                    if matches!(
                        name.as_str(),
                        "tags" | "IsHidden" | "AllowOverlay" | "AllowDesktopConfig"
                    ) {
                        continue;
                    }
                    match fields
                        .iter_mut()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    {
                        Some(slot) => slot.1 = value.clone(),
                        None => fields.push((name.clone(), value.clone())),
                    }
                }
            }
            entries[at] = (key, merged);
            Ok(true)
        }
        None => {
            // Steam keys entries by their position, as a decimal string.
            let index = entries.len();
            entries.push((index.to_string(), shortcut.to_value()));
            Ok(false)
        }
    }
}

/// An empty `shortcuts.vdf`, for the case where there is not one yet.
pub fn empty_document() -> Value {
    Value::Map(vec![("shortcuts".into(), Value::Map(vec![]))])
}

/// Every Steam account on this machine that could hold shortcuts.
///
/// More than one is normal -- a second account, or a leftover from a reinstall
/// -- and guessing wrong writes a shortcut into a library nobody opens, which
/// looks exactly like the feature not working.
pub fn shortcut_files(steam_dir: &Path) -> Vec<PathBuf> {
    let userdata = steam_dir.join("userdata");
    let Ok(entries) = std::fs::read_dir(&userdata) else {
        return Vec::new();
    };
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().to_string();
        // "0" and "anonymous" are Steam's own, not a person's.
        if !id.chars().all(|c| c.is_ascii_digit()) || id == "0" {
            continue;
        }
        let config = entry.path().join("config");
        if config.is_dir() {
            found.insert(id, config.join("shortcuts.vdf"));
        }
    }
    found.into_values().collect()
}

/// Read a shortcuts file, or hand back an empty document when there is none.
pub fn load(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(empty_document()),
        Ok(bytes) => parse(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(empty_document()),
        Err(e) => Err(anyhow!("{}: {e}", path.display())),
    }
}

/// Write a shortcuts file, keeping a copy of what was there.
///
/// The backup is the point. This is Steam's file and it holds shortcuts the
/// launcher knows nothing about; if the write is wrong, "restore the .bak" has
/// to be an answer that exists.
pub fn save(path: &Path, document: &Value) -> Result<()> {
    let bytes = serialise(document)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.is_file() {
        let _ = std::fs::copy(path, path.with_extension("vdf.bak"));
    }
    let temporary = path.with_extension("vdf.new");
    std::fs::write(&temporary, &bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Whether Steam is running, because it rewrites `shortcuts.vdf` from memory
/// when it exits and would discard anything written underneath it.
pub fn is_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(pid) = name.to_str() else {
            return false;
        };
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        // `comm` is the executable name only, so this cannot be fooled by a
        // command line that merely mentions Steam -- which is exactly what the
        // launcher's own process looks like.
        std::fs::read_to_string(entry.path().join("comm"))
            .map(|comm| comm.trim() == "steam")
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shortcuts.vdf with one entry, built the way Steam builds one.
    fn sample() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(TYPE_MAP);
        out.extend_from_slice(b"shortcuts\0");
        out.push(TYPE_MAP);
        out.extend_from_slice(b"0\0");
        out.push(TYPE_STRING);
        out.extend_from_slice(b"AppName\0Someone Elses Game\0");
        out.push(TYPE_STRING);
        out.extend_from_slice(b"Exe\0\"/usr/bin/thing\"\0");
        out.push(TYPE_INT32);
        out.extend_from_slice(b"IsHidden\0");
        out.extend_from_slice(&0i32.to_le_bytes());
        out.push(TYPE_MAP);
        out.extend_from_slice(b"tags\0");
        out.push(TYPE_STRING);
        out.extend_from_slice(b"0\0favourite\0");
        out.push(END_MAP);
        out.push(END_MAP);
        out.push(END_MAP);
        out
    }

    #[test]
    fn a_document_survives_a_round_trip_byte_for_byte() {
        let bytes = sample();
        let document = parse(&bytes).expect("parses");
        assert_eq!(serialise(&document).expect("serialises"), bytes);
    }

    #[test]
    fn nested_values_come_back_as_written() {
        let document = parse(&sample()).expect("parses");
        let entry = document.get("shortcuts").unwrap().get("0").unwrap();
        assert_eq!(
            entry.get("AppName").unwrap().as_str(),
            Some("Someone Elses Game")
        );
        assert_eq!(entry.get("IsHidden"), Some(&Value::Int(0)));
        assert_eq!(
            entry.get("tags").unwrap().get("0").unwrap().as_str(),
            Some("favourite")
        );
        assert_eq!(
            entry.get("appname").unwrap().as_str(),
            Some("Someone Elses Game"),
            "Steam has spelled these keys both ways"
        );
    }

    /// The file belongs to Steam and holds shortcuts we know nothing about.
    #[test]
    fn adding_a_shortcut_leaves_every_other_entry_untouched() {
        let mut document = parse(&sample()).expect("parses");
        let shortcut = Shortcut::new(
            "Test Title",
            Path::new("/opt/xgdk/bin/xgdk"),
            Path::new("/opt/xgdk/bin"),
            "run 9ZZTESTGAME1",
        );
        assert!(
            !upsert(&mut document, &shortcut).expect("adds"),
            "a new entry"
        );

        let shortcuts = document.get("shortcuts").unwrap();
        assert_eq!(shortcuts.as_map().unwrap().len(), 2);
        let theirs = shortcuts.get("0").unwrap();
        assert_eq!(
            theirs.get("AppName").unwrap().as_str(),
            Some("Someone Elses Game")
        );
        assert_eq!(
            theirs.get("tags").unwrap().get("0").unwrap().as_str(),
            Some("favourite")
        );

        let ours = shortcuts.get("1").unwrap();
        assert_eq!(ours.get("AppName").unwrap().as_str(), Some("Test Title"));
        assert_eq!(
            ours.get("Exe").unwrap().as_str(),
            Some("\"/opt/xgdk/bin/xgdk\"")
        );
        assert_eq!(
            ours.get("LaunchOptions").unwrap().as_str(),
            Some("run 9ZZTESTGAME1")
        );
    }

    /// Adding the same title twice must update the entry, not duplicate it --
    /// and must not stamp on settings the person changed themselves.
    #[test]
    fn adding_the_same_title_twice_updates_and_keeps_their_settings() {
        let mut document = empty_document();
        let first = Shortcut::new(
            "Test Title",
            Path::new("/old/xgdk"),
            Path::new("/old"),
            "run 9ZZTESTGAME1",
        );
        assert!(!upsert(&mut document, &first).expect("adds"));

        // Stand in for a person turning the overlay off and tagging it.
        let Value::Map(root) = &mut document else {
            unreachable!()
        };
        let Value::Map(entries) = &mut root[0].1 else {
            unreachable!()
        };
        let Value::Map(fields) = &mut entries[0].1 else {
            unreachable!()
        };
        for (key, value) in fields.iter_mut() {
            match key.as_str() {
                "AllowOverlay" => *value = Value::Int(0),
                "tags" => *value = Value::Map(vec![("0".into(), Value::Str("xgdk".into()))]),
                _ => {}
            }
        }

        let moved = Shortcut::new(
            "Test Title",
            Path::new("/new/xgdk"),
            Path::new("/new"),
            "run 9ZZTESTGAME1",
        );
        assert!(
            upsert(&mut document, &moved).expect("updates"),
            "replaced, not added"
        );

        let shortcuts = document.get("shortcuts").unwrap();
        assert_eq!(shortcuts.as_map().unwrap().len(), 1, "not duplicated");
        let entry = shortcuts.get("0").unwrap();
        assert_eq!(entry.get("Exe").unwrap().as_str(), Some("\"/new/xgdk\""));
        assert_eq!(
            entry.get("AllowOverlay"),
            Some(&Value::Int(0)),
            "their setting stands"
        );
        assert_eq!(
            entry.get("tags").unwrap().get("0").unwrap().as_str(),
            Some("xgdk")
        );
    }

    /// A path with a space in it is the normal case, and an unquoted one fails
    /// to launch with no error anyone can see.
    #[test]
    fn paths_are_quoted() {
        let shortcut = Shortcut::new(
            "Test Title",
            Path::new("/home/someone/My Games/xgdk"),
            Path::new("/home/someone/My Games"),
            "",
        );
        assert_eq!(shortcut.exe, "\"/home/someone/My Games/xgdk\"");
        assert_eq!(shortcut.start_dir, "\"/home/someone/My Games\"");
    }

    /// Steam derives the id this way, so artwork and `steam://` URLs only line
    /// up if we derive the same number.
    #[test]
    fn the_app_id_is_derived_the_way_steam_derives_it() {
        let id = shortcut_app_id("\"/opt/xgdk/bin/xgdk\"", "Test Title");
        assert_eq!(id & 0x8000_0000, 0x8000_0000, "the high bit is always set");
        assert_eq!(
            id,
            shortcut_app_id("\"/opt/xgdk/bin/xgdk\"", "Test Title"),
            "stable"
        );
        assert_ne!(id, shortcut_app_id("\"/opt/xgdk/bin/xgdk\"", "Other Title"));
    }

    #[test]
    fn crc32_matches_the_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    /// A file we do not fully understand is one we must not rewrite.
    #[test]
    fn an_unknown_value_type_is_refused_rather_than_skipped() {
        let mut bytes = Vec::new();
        bytes.push(TYPE_MAP);
        bytes.extend_from_slice(b"shortcuts\0");
        bytes.push(0x07);
        bytes.extend_from_slice(b"mystery\0");
        assert!(parse(&bytes).is_err());

        assert!(parse(b"").is_err());
        assert!(parse(b"\x01not-a-map\0").is_err());
    }

    #[test]
    fn a_missing_file_reads_as_an_empty_document() {
        let document = load(Path::new("/nonexistent/shortcuts.vdf")).expect("no file is fine");
        assert_eq!(document, empty_document());
        assert!(document
            .get("shortcuts")
            .unwrap()
            .as_map()
            .unwrap()
            .is_empty());
    }
}
