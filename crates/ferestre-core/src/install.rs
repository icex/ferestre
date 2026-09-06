//! What is installed, and which build of it.
//!
//! A decrypted title on disk does not reliably say what version it is: the
//! package version is a property of the MSIXVC, not of the files that come out
//! of it, and the anonymous catalog reports `Version` as `"0"` for everything.
//! So the launcher records what it installed, at the moment it installs it.
//!
//! The recorded thing was the set of **content ids**, on the belief that a
//! content id changes when a package is rebuilt. **It does not.** Measured
//! against 26 published patch plans spanning 26 distinct builds of three
//! titles: three content ids, one per title, identical across every build. A
//! content id names the title's package; the *version* names the build, and the
//! delivery path is `/{ContentId}/{VersionId}/`.
//!
//! So the update key is the package version, and the content ids are kept only
//! because they are the path component a download needs. What the anonymous
//! catalog cannot supply is the *available* version -- it reports `"0"` for
//! everything -- so until the authenticated update endpoint is wired up, the
//! honest answer to "is there an update" is "cannot tell", and that is what
//! this returns. See docs/ROADMAP.md for the endpoint and the plan format.
//!
//! The install does not have to be taken on trust, either. The client leaves the
//! package header on disk as `.xodus-streaming.msixvc`, and the GUID in it *is*
//! the catalog's content id -- so a title installed before this launcher existed
//! can be adopted exactly, from a 4 KiB read, with nothing to assume and nothing
//! to ask the user.

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
    /// The package's content ids. **Not** the update key -- they are identical
    /// across every build of a title -- but they are the path component a
    /// download is fetched by, so they are worth having written down.
    #[serde(default)]
    pub content_ids: Vec<String>,
    /// RFC 3339, as the caller spells it. Nothing does arithmetic on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    /// The `Identity Version` out of the package manifest at install time.
    ///
    /// Not the update key -- two builds of a title can share a version string,
    /// and the catalog will not tell us the current one, so this cannot answer
    /// "is there something newer". It is here for two other jobs: it is the only
    /// version a person can read, and comparing it against the manifest on disk
    /// later says whether the tree changed underneath the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
}

impl Record {
    /// Whether the directory this record describes is still there.
    ///
    /// People move and delete game directories without telling a launcher, and a
    /// record for a title that is gone would keep it in the Installed list and
    /// could offer an update for something not on disk.
    pub fn is_stale(&self) -> bool {
        !self.dir.is_dir()
    }

    /// Whether the tree on disk still looks like what was recorded.
    ///
    /// `None` when either side has no version to compare, which is the common
    /// case and not a problem. `Some(false)` means something reinstalled or
    /// updated the title outside the launcher, so the recorded content ids
    /// describe a build that is no longer there.
    pub fn matches_disk(&self, on_disk: Option<&str>) -> Option<bool> {
        match (self.package_version.as_deref(), on_disk) {
            (Some(recorded), Some(found)) => Some(recorded == found),
            _ => None,
        }
    }

    /// Whether a newer build than this one is available.
    ///
    /// `available` is the version the service is offering. `None` -- meaning
    /// "cannot tell" -- whenever either side is unknown, which today is always,
    /// because the anonymous catalog reports every version as `"0"` and nothing
    /// yet asks the authenticated endpoint that knows.
    ///
    /// That is deliberate and it is the whole point of this signature. The
    /// previous version compared content ids, which are identical across every
    /// build of a title, so it could only ever answer "up to date" -- a
    /// confident wrong answer to the one question this module exists to ask.
    /// "Cannot tell" is worth more than that.
    pub fn update_available(&self, available: Option<&str>) -> Option<bool> {
        let installed = self.package_version.as_deref()?;
        let available = available?;
        if installed.is_empty() || available.is_empty() {
            return None;
        }
        Some(installed != available)
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

/// An RFC 3339 timestamp for now, in UTC.
///
/// Written by hand rather than by pulling in a date crate, because the only
/// thing this project does with a timestamp is show it and sort by it, and the
/// civil-from-days conversion is a known twenty lines. `None` if the clock is
/// before 1970, which is not a case worth handling beyond not lying about it.
pub fn now_rfc3339() -> Option<String> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(rfc3339(seconds))
}

/// Format Unix seconds as RFC 3339 UTC. Howard Hinnant's civil_from_days.
fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// The package header the client leaves beside an installed title.
const CONTAINER: &str = ".xodus-streaming.msixvc";

/// The content id of what is actually on disk.
///
/// Read out of the XVD header of the container the client keeps after a
/// streaming install. This is the same GUID the catalog calls `ContentId`, and
/// that is not a coincidence to be re-checked each time: the client passes this
/// very field to the licensing service to obtain the key, so a title that
/// launches at all has a header whose GUID the service recognised.
///
/// `None` -- never a guess -- when there is no container, when it does not
/// start with the XVD magic, or when it is too short. A wrong answer here would
/// become a wrong "up to date", which is the one outcome worth failing to avoid.
pub fn content_id(install_dir: &Path) -> Option<String> {
    use std::io::Read;

    // Offsets into the XVD header, from the client's own parser: the magic at
    // 0x200 opens the header, and VDUID is the first GUID after it.
    const MAGIC_AT: usize = 0x200;
    const MAGIC: &[u8; 8] = b"msft-xvd";
    const VDUID_AT: usize = 0x220;
    const NEEDED: usize = VDUID_AT + 16;

    let mut file = std::fs::File::open(install_dir.join(CONTAINER)).ok()?;
    let mut head = vec![0u8; NEEDED];
    file.read_exact(&mut head).ok()?;
    if &head[MAGIC_AT..MAGIC_AT + 8] != MAGIC {
        return None;
    }
    Some(guid_le(&head[VDUID_AT..VDUID_AT + 16]))
}

/// Format 16 bytes as a GUID the way Microsoft stores one: the first three
/// fields little-endian, the rest as written.
fn guid_le(b: &[u8]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{}",
        b[3],
        b[2],
        b[1],
        b[0],
        b[5],
        b[4],
        b[7],
        b[6],
        b[8],
        b[9],
        b[10..16]
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    )
}

/// Build a record for a title already on disk that nothing recorded.
///
/// Exact, not assumed: everything comes from the install itself. Returns `None`
/// when the directory has no readable container, because a record claiming a
/// content id it did not read would be worse than no record at all -- it would
/// answer "up to date" to a question it cannot answer.
pub fn adopt(product_id: &str, dir: &Path) -> Option<Record> {
    let content_id = content_id(dir)?;
    Some(Record {
        product_id: product_id.to_string(),
        dir: dir.to_path_buf(),
        content_ids: vec![content_id],
        // Not `now`: this records what is on disk, and pretending to know when
        // it arrived would be inventing a fact.
        installed_at: None,
        package_version: package_version(dir),
    })
}

/// The `Identity Version` from a package manifest, if the directory has one.
///
/// Deliberately not an XML parser. The manifest is Microsoft's, it is one
/// element deep for this purpose, and a dependency to read one attribute would
/// be the wrong trade. Attribute order is not assumed, because it is not
/// guaranteed and assuming it is exactly the kind of thing that works on the
/// two titles you tested with.
pub fn package_version(install_dir: &Path) -> Option<String> {
    // Windows is case-insensitive and packages have shipped both spellings.
    let manifest = ["appxmanifest.xml", "AppxManifest.xml"]
        .iter()
        .map(|name| install_dir.join(name))
        .find(|path| path.is_file())?;
    let text = std::fs::read_to_string(manifest).ok()?;

    let start = text.find("<Identity")?;
    let end = text[start..].find('>')? + start;
    let element = &text[start..end];
    let at = element.find("Version=")?;
    let rest = &element[at + "Version=".len()..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let close = value.find(quote)?;
    let version = value[..close].trim();
    (!version.is_empty()).then(|| version.to_string())
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
            package_version: Some("1.26.4501.0".into()),
        }
    }

    /// The correction this signature exists for. Content ids do not change
    /// between builds -- 26 published plans across 26 builds of three titles
    /// carry three content ids, one per title -- so comparing them answered
    /// "up to date" to every real update. A version comparison answers it.
    #[test]
    fn a_newer_version_is_an_update() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.update_available(Some("1.26.4501.0")), Some(false));
        assert_eq!(installed.update_available(Some("1.26.4600.0")), Some(true));
        // Downgrades count as different, because "not what I have" is the
        // question; a rollback still needs the files fetched.
        assert_eq!(installed.update_available(Some("1.26.4403.0")), Some(true));
    }

    /// Neither "up to date" nor "stale" is honest with nothing to compare, and
    /// a launcher that guesses either way either nags or hides a real update.
    /// Today the available version is never known, so this is the normal case.
    #[test]
    fn nothing_to_compare_is_unknown_not_a_verdict() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.update_available(None), None);
        assert_eq!(installed.update_available(Some("")), None);

        let mut versionless = installed.clone();
        versionless.package_version = None;
        assert_eq!(versionless.update_available(Some("1.26.4600.0")), None);
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
        let dir =
            std::env::temp_dir().join(format!("ferestre-install-test-{}", std::process::id()));
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
