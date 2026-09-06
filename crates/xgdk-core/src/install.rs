//! What is installed, and which build of it.
//!
//! A decrypted title on disk does not reliably say what version it is: the
//! package version is a property of the MSIXVC, not of the files that come out
//! of it, and the anonymous catalog reports `Version` as `"0"` for everything.
//! So the launcher records what it installed, at the moment it installs it.
//!
//! The recorded thing is the set of **content ids**, because that is what
//! actually changes when a package is rebuilt. An update is then a comparison
//! rather than a download: the catalog lists a content id we do not have.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Written beside the state directory, one file per title.
const DIR: &str = "installed";

/// What was installed, when, and from which build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub product_id: String,
    /// Where it went. Kept so a record can be recognised as stale when someone
    /// moves or deletes the directory behind the launcher's back.
    pub dir: PathBuf,
    /// The catalog's content ids at install time. Empty when the install
    /// predates this record or happened outside the launcher, which is not an
    /// error -- it only means updates cannot be detected for that title until
    /// it is installed again.
    #[serde(default)]
    pub content_ids: Vec<String>,
    /// RFC 3339, as the caller spells it. Nothing does arithmetic on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
}

impl Record {
    /// Whether the catalog is now offering a build this install does not have.
    ///
    /// Unknown -- rather than "no" -- when either side has nothing to compare.
    /// An install recorded before content ids were kept must not be reported as
    /// up to date on no evidence; a title whose catalog entry has not been
    /// fetched yet must not be reported as stale.
    pub fn update_available(&self, catalog: &[String]) -> Option<bool> {
        if self.content_ids.is_empty() || catalog.is_empty() {
            return None;
        }
        Some(catalog.iter().any(|id| !self.content_ids.contains(id)))
    }
}

/// Where a title's record lives.
///
/// `None` for anything that is not a product id. The id reaches here from a
/// network service and from a recipe file, and it ends up in a path.
pub fn record_path(state_dir: &Path, product_id: &str) -> Option<PathBuf> {
    if product_id.is_empty()
        || !product_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(
        state_dir
            .join(DIR)
            .join(format!("{}.json", product_id.to_ascii_uppercase())),
    )
}

pub fn load(state_dir: &Path, product_id: &str) -> Option<Record> {
    let path = record_path(state_dir, product_id)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(state_dir: &Path, record: &Record) -> Result<()> {
    let path = record_path(state_dir, &record.product_id)
        .ok_or_else(|| anyhow!("not a usable product id: {:?}", record.product_id))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(record)?)?;
    Ok(())
}

pub fn forget(state_dir: &Path, product_id: &str) -> Result<()> {
    if let Some(path) = record_path(state_dir, product_id) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Every recorded install. Unreadable files are skipped: a record is a
/// convenience, and one corrupt file must not hide the rest.
pub fn all(state_dir: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(DIR)) else {
        return Vec::new();
    };
    let mut records: Vec<Record> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    records.sort_by(|a, b| a.product_id.cmp(&b.product_id));
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(content_ids: &[&str]) -> Record {
        Record {
            product_id: "9ZZTESTGAME1".into(),
            dir: PathBuf::from("/games/test"),
            content_ids: content_ids.iter().map(|s| s.to_string()).collect(),
            installed_at: Some("2026-09-06T09:00:00Z".into()),
        }
    }

    #[test]
    fn a_new_content_id_in_the_catalog_is_an_update() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.update_available(&["content-a".into()]), Some(false));
        assert_eq!(installed.update_available(&["content-b".into()]), Some(true));
    }

    /// A title split across packages is stale if any one of them was rebuilt.
    #[test]
    fn one_rebuilt_package_out_of_several_is_an_update() {
        let installed = record(&["content-a", "content-b"]);
        assert_eq!(
            installed.update_available(&["content-a".into(), "content-b".into()]),
            Some(false)
        );
        assert_eq!(
            installed.update_available(&["content-a".into(), "content-c".into()]),
            Some(true)
        );
    }

    /// Neither "up to date" nor "stale" is honest with nothing to compare, and
    /// a launcher that guesses either way either nags or hides a real update.
    #[test]
    fn nothing_to_compare_is_unknown_not_a_verdict() {
        assert_eq!(record(&[]).update_available(&["content-a".into()]), None);
        assert_eq!(record(&["content-a"]).update_available(&[]), None);
    }

    #[test]
    fn a_product_id_that_is_not_one_never_becomes_a_path() {
        let state = Path::new("/state");
        assert!(record_path(state, "../../etc/passwd").is_none());
        assert!(record_path(state, "").is_none());
        assert_eq!(
            record_path(state, "9zztestgame1"),
            Some(PathBuf::from("/state/installed/9ZZTESTGAME1.json"))
        );
    }

    #[test]
    fn a_record_round_trips_and_can_be_forgotten() {
        let dir = std::env::temp_dir().join(format!("xgdk-install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(load(&dir, "9ZZTESTGAME1").is_none());
        assert!(all(&dir).is_empty());

        let written = record(&["content-a"]);
        save(&dir, &written).expect("writes");
        assert_eq!(load(&dir, "9ZZTESTGAME1"), Some(written.clone()));
        assert_eq!(all(&dir), vec![written]);

        forget(&dir, "9ZZTESTGAME1").expect("forgets");
        assert!(load(&dir, "9ZZTESTGAME1").is_none());
        forget(&dir, "9ZZTESTGAME1").expect("forgetting twice is not an error");

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }
}
