//! `neural-rendering.toml`: which games have neural rendering, how it was
//! installed, and which anti-cheats the person acknowledged for each game.
//!
//! ```toml
//! [titles.LOCAL-ABC]
//! enabled = true
//! route = "optiscaler"
//! model = "CC_SMALL_FP8"
//! package = "0.0.9"
//! arch = "x86_64"
//! install-dir = "/games/Abc/bin"
//!
//! [acknowledged]
//! LOCAL-ABC = ["Easy Anti-Cheat"]
//! ```
//!
//! Keys are product ids in upper case. Unknown top-level tables survive a
//! write. A file that cannot be read is an error naming it, never a blank
//! slate, so a damaged file never loses what is installed where.

use super::{Arch, Network, Route};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FILE: &str = "neural-rendering.toml";

/// Neural rendering for one game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Entry {
    pub enabled: bool,
    pub route: Route,
    pub model: Network,
    /// The package version installed.
    pub package: String,
    pub arch: Arch,
    /// The executable's folder the mod went into.
    pub install_dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    titles: BTreeMap<String, Entry>,
    #[serde(default)]
    acknowledged: BTreeMap<String, BTreeSet<String>>,
    /// Whatever a newer build added.
    #[serde(flatten)]
    other: toml::Table,
}

pub fn file(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

fn key(key: &str) -> String {
    key.to_ascii_uppercase()
}

fn read(config_dir: &Path) -> Result<Document> {
    let path = file(config_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Document::default()),
        Err(e) => bail!("{}: {e}", path.display()),
    };
    toml::from_str(&text).map_err(|e| {
        anyhow!(
            "{}: {e}\nfix the file; it records where neural rendering is installed",
            path.display()
        )
    })
}

fn write(config_dir: &Path, document: &Document) -> Result<()> {
    crate::cache::write(
        &file(config_dir),
        toml::to_string_pretty(document)?.as_bytes(),
    )
}

/// Read, change and write back in one go.
fn update(config_dir: &Path, change: impl FnOnce(&mut Document)) -> Result<()> {
    let mut document = read(config_dir)?;
    change(&mut document);
    write(config_dir, &document)
}

pub fn get(config_dir: &Path, product_id: &str) -> Result<Option<Entry>> {
    Ok(read(config_dir)?.titles.remove(&key(product_id)))
}

pub fn put(config_dir: &Path, product_id: &str, entry: &Entry) -> Result<()> {
    update(config_dir, |d| {
        d.titles.insert(key(product_id), entry.clone());
    })
}

/// The anti-cheats acknowledged for a game, by `Detection::name`.
pub fn acknowledged(config_dir: &Path, product_id: &str) -> Result<BTreeSet<String>> {
    Ok(read(config_dir)?
        .acknowledged
        .remove(&key(product_id))
        .unwrap_or_default())
}

pub fn acknowledge(config_dir: &Path, product_id: &str, names: &[&str]) -> Result<()> {
    update(config_dir, |d| {
        d.acknowledged
            .entry(key(product_id))
            .or_default()
            .extend(names.iter().map(|n| n.to_string()));
    })
}

/// The entry and any new acknowledgements, in one write: an acknowledgement
/// is only ever stored together with a successful enable.
pub fn record(
    config_dir: &Path,
    product_id: &str,
    entry: &Entry,
    acknowledged: &[&str],
) -> Result<()> {
    update(config_dir, |d| {
        let k = key(product_id);
        d.titles.insert(k.clone(), entry.clone());
        if !acknowledged.is_empty() {
            d.acknowledged
                .entry(k)
                .or_default()
                .extend(acknowledged.iter().map(|n| n.to_string()));
        }
    })
}

/// Drop a game's entry; its acknowledgements stay.
pub fn forget(config_dir: &Path, product_id: &str) -> Result<()> {
    let k = key(product_id);
    if !read(config_dir)?.titles.contains_key(&k) {
        return Ok(());
    }
    update(config_dir, |d| {
        d.titles.remove(&k);
    })
}

/// Drop everything kept about a game, its acknowledgements included: for a
/// game Ferestre forgets. A local game's id comes from its name, so another
/// game added later under the same name must not inherit them.
pub fn forget_game(config_dir: &Path, product_id: &str) -> Result<()> {
    let k = key(product_id);
    let document = read(config_dir)?;
    if !document.titles.contains_key(&k) && !document.acknowledged.contains_key(&k) {
        return Ok(());
    }
    update(config_dir, |d| {
        d.titles.remove(&k);
        d.acknowledged.remove(&k);
    })
}

pub fn all(config_dir: &Path) -> Result<BTreeMap<String, Entry>> {
    Ok(read(config_dir)?.titles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str) -> Entry {
        Entry {
            enabled: true,
            route: Route::ReshadeFeederVulkan,
            model: Network::SilverAardwoldOptimized,
            package: "0.0.9".into(),
            arch: Arch::I686,
            install_dir: PathBuf::from(dir),
        }
    }

    #[test]
    fn an_entry_round_trips_and_other_titles_survive_a_write() {
        let config = tempfile::tempdir().unwrap();
        let c = config.path();
        assert_eq!(get(c, "local-a").unwrap(), None);
        put(c, "local-a", &entry("/games/a")).unwrap();
        put(c, "9NBLGGH4R315", &entry("/games/b")).unwrap();
        assert_eq!(get(c, "LOCAL-A").unwrap(), Some(entry("/games/a")));
        let text = std::fs::read_to_string(file(c)).unwrap();
        assert!(text.contains("[titles.LOCAL-A]"), "{text}");
        assert!(text.contains("route = \"reshade-feeder-vulkan\""), "{text}");
        assert!(text.contains("install-dir = \"/games/a\""), "{text}");

        let mut off = entry("/games/a");
        off.enabled = false;
        put(c, "local-a", &off).unwrap();
        let all = all(c).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all["LOCAL-A"], off);
        assert_eq!(all["9NBLGGH4R315"], entry("/games/b"));

        // What a newer build added is kept.
        let mut text = std::fs::read_to_string(file(c)).unwrap();
        text.push_str("\n[future]\nx = 1\n");
        std::fs::write(file(c), text).unwrap();
        forget(c, "9NBLGGH4R315").unwrap();
        let text = std::fs::read_to_string(file(c)).unwrap();
        assert!(text.contains("[future]"), "{text}");
        assert_eq!(all_keys(c), vec!["LOCAL-A"]);
    }

    fn all_keys(c: &Path) -> Vec<String> {
        all(c).unwrap().into_keys().collect()
    }

    #[test]
    fn acknowledgements_outlive_forgetting_the_entry() {
        let config = tempfile::tempdir().unwrap();
        let c = config.path();
        put(c, "local-a", &entry("/games/a")).unwrap();
        acknowledge(c, "local-a", &["Easy Anti-Cheat"]).unwrap();
        acknowledge(c, "LOCAL-A", &["BattlEye", "Easy Anti-Cheat"]).unwrap();
        forget(c, "local-a").unwrap();
        assert_eq!(get(c, "local-a").unwrap(), None);
        assert_eq!(
            acknowledged(c, "local-a").unwrap(),
            BTreeSet::from(["BattlEye".to_string(), "Easy Anti-Cheat".to_string()])
        );
        assert!(acknowledged(c, "local-b").unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_game_drops_its_acknowledgements_too() {
        let config = tempfile::tempdir().unwrap();
        let c = config.path();
        record(c, "local-a", &entry("/games/a"), &["Easy Anti-Cheat"]).unwrap();
        acknowledge(c, "local-b", &["BattlEye"]).unwrap();
        forget_game(c, "LOCAL-A").unwrap();
        assert_eq!(get(c, "local-a").unwrap(), None);
        assert!(acknowledged(c, "local-a").unwrap().is_empty());
        assert_eq!(acknowledged(c, "local-b").unwrap().len(), 1);
        // Only acknowledgements left: they go too.
        forget_game(c, "local-b").unwrap();
        assert!(acknowledged(c, "local-b").unwrap().is_empty());
        // Nothing kept, nothing written.
        let config = tempfile::tempdir().unwrap();
        forget_game(config.path(), "local-a").unwrap();
        assert!(!file(config.path()).exists());
    }

    #[test]
    fn record_writes_the_entry_and_its_acknowledgements_together() {
        let config = tempfile::tempdir().unwrap();
        let c = config.path();
        record(c, "local-a", &entry("/games/a"), &["Easy Anti-Cheat"]).unwrap();
        assert_eq!(get(c, "local-a").unwrap(), Some(entry("/games/a")));
        assert_eq!(
            acknowledged(c, "local-a").unwrap(),
            BTreeSet::from(["Easy Anti-Cheat".to_string()])
        );
        record(c, "local-a", &entry("/games/a2"), &[]).unwrap();
        assert_eq!(acknowledged(c, "local-a").unwrap().len(), 1);
        assert_eq!(get(c, "local-a").unwrap(), Some(entry("/games/a2")));
    }

    #[test]
    fn an_unreadable_file_is_an_error_naming_it_and_is_not_overwritten() {
        let config = tempfile::tempdir().unwrap();
        let c = config.path();
        std::fs::write(file(c), "titles = [not toml").unwrap();
        let err = get(c, "local-a").unwrap_err().to_string();
        assert!(err.contains("neural-rendering.toml"), "{err}");
        assert!(all(c).is_err());
        assert!(acknowledged(c, "local-a").is_err());
        assert!(put(c, "local-a", &entry("/games/a")).is_err());
        assert!(record(c, "local-a", &entry("/games/a"), &["BattlEye"]).is_err());
        assert!(forget(c, "local-a").is_err());
        assert!(acknowledge(c, "local-a", &["BattlEye"]).is_err());
        assert_eq!(
            std::fs::read_to_string(file(c)).unwrap(),
            "titles = [not toml"
        );
    }
}
