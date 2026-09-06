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
    /// The catalog's content ids at install time. Empty when the install
    /// predates this record or happened outside the launcher, which is not an
    /// error -- it only means updates cannot be detected for that title until
    /// it is installed again.
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

    #[test]
    fn a_new_content_id_in_the_catalog_is_an_update() {
        let installed = record(&["content-a"]);
        assert_eq!(
            installed.update_available(&["content-a".into()]),
            Some(false)
        );
        assert_eq!(
            installed.update_available(&["content-b".into()]),
            Some(true)
        );
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

    /// The manifest is Microsoft's and attribute order is not guaranteed, so the
    /// reader must not assume the shape the two titles here happen to have.
    #[test]
    fn the_package_version_is_read_whatever_the_attribute_order() {
        let dir = std::env::temp_dir().join(format!("xgdk-manifest-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        let write = |body: &str| {
            std::fs::write(dir.join("appxmanifest.xml"), body).expect("writes");
            package_version(&dir)
        };

        // The real shape, from a shipped title.
        assert_eq!(
            write(
                r#"<?xml version="1.0"?><Package><Identity Name="Microsoft.MinecraftUWP" Publisher="CN=Microsoft Corporation" Version="1.26.4501.0" ProcessorArchitecture="x64" /></Package>"#
            ),
            Some("1.26.4501.0".into())
        );
        // Version first, single quotes, and extra whitespace.
        assert_eq!(
            write("<Package><Identity   Version='2.0.1.0'  Name=\"X\" /></Package>"),
            Some("2.0.1.0".into())
        );
        // A package with no version, and one with an empty one.
        assert_eq!(write(r#"<Package><Identity Name="X" /></Package>"#), None);
        assert_eq!(
            write(r#"<Package><Identity Version="" Name="X" /></Package>"#),
            None
        );
        // Not a manifest at all.
        assert_eq!(write("not xml"), None);
        // A later element that mentions Version must not be mistaken for it.
        assert_eq!(
            write(r#"<Package><Identity Name="X" /><Dependency Version="9.9.9.9" /></Package>"#),
            None,
            "only the Identity element names the package version"
        );

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    /// Hand-rolled date maths earns its tests. Vectors chosen for the cases that
    /// break naive conversions: the epoch, a leap day, and a century that is not
    /// a leap year on the other side of one that is.
    #[test]
    fn timestamps_are_rfc_3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1), "1970-01-01T00:00:01Z");
        assert_eq!(
            rfc3339(951_782_400),
            "2000-02-29T00:00:00Z",
            "2000 is a leap year"
        );
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_788_690_600), "2026-09-06T10:30:00Z");
        assert_eq!(
            rfc3339(4_107_542_400),
            "2100-03-01T00:00:00Z",
            "2100 is not"
        );
        assert_eq!(rfc3339(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(rfc3339(86_400), "1970-01-02T00:00:00Z");

        let now = now_rfc3339().expect("the clock is after 1970");
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z') && now.contains('T'), "{now}");
    }

    /// A synthetic container: the magic where the header starts and a known
    /// GUID where VDUID sits.
    fn container(dir: &Path, guid: &[u8; 16], magic: &[u8; 8]) {
        let mut bytes = vec![0u8; 0x1000];
        bytes[0x200..0x208].copy_from_slice(magic);
        bytes[0x220..0x230].copy_from_slice(guid);
        std::fs::write(dir.join(CONTAINER), bytes).expect("writes the container");
    }

    /// The bytes of 7792d9ce-355a-493c-afbd-768f4a77c3b0 as Microsoft stores
    /// them -- first three fields little-endian. Taken from a real install.
    const REAL_GUID: [u8; 16] = [
        0xce, 0xd9, 0x92, 0x77, 0x5a, 0x35, 0x3c, 0x49, 0xaf, 0xbd, 0x76, 0x8f, 0x4a, 0x77, 0xc3,
        0xb0,
    ];

    #[test]
    fn the_content_id_is_read_from_the_package_header() {
        let dir = std::env::temp_dir().join(format!("xgdk-container-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        container(&dir, &REAL_GUID, b"msft-xvd");
        assert_eq!(
            content_id(&dir).as_deref(),
            Some("7792d9ce-355a-493c-afbd-768f4a77c3b0"),
            "the byte order is Microsoft's, not RFC 4122's"
        );

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    /// Every way of not knowing must produce `None`. A guess here becomes a
    /// wrong "up to date", which is the one answer worth failing to avoid.
    #[test]
    fn anything_that_is_not_a_package_header_yields_nothing() {
        let dir = std::env::temp_dir().join(format!("xgdk-container-neg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        assert_eq!(content_id(&dir), None, "no container at all");

        container(&dir, &REAL_GUID, b"not-xvd!");
        assert_eq!(content_id(&dir), None, "wrong magic");

        std::fs::write(dir.join(CONTAINER), vec![0u8; 0x100]).expect("writes");
        assert_eq!(content_id(&dir), None, "truncated before the header");

        std::fs::write(dir.join(CONTAINER), b"").expect("writes");
        assert_eq!(content_id(&dir), None, "empty");

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    /// Adoption is exact or it does not happen. It must never invent a content
    /// id, and it must not claim an install date it does not know.
    #[test]
    fn adoption_reads_the_install_or_declines() {
        let dir = std::env::temp_dir().join(format!("xgdk-adopt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        assert!(
            adopt("9ZZTESTGAME1", &dir).is_none(),
            "nothing to read, nothing to claim"
        );

        container(&dir, &REAL_GUID, b"msft-xvd");
        std::fs::write(
            dir.join("appxmanifest.xml"),
            r#"<Package><Identity Name="X" Version="1.26.4501.0" /></Package>"#,
        )
        .expect("writes");

        let adopted = adopt("9ZZTESTGAME1", &dir).expect("adopts");
        assert_eq!(
            adopted.content_ids,
            vec!["7792d9ce-355a-493c-afbd-768f4a77c3b0"]
        );
        assert_eq!(adopted.package_version.as_deref(), Some("1.26.4501.0"));
        assert_eq!(adopted.dir, dir);
        assert_eq!(
            adopted.installed_at, None,
            "the install date is not knowable from disk, so it is not invented"
        );

        // The point of all of it: an adopted record answers the update question.
        assert_eq!(
            adopted.update_available(&["7792d9ce-355a-493c-afbd-768f4a77c3b0".into()]),
            Some(false)
        );
        assert_eq!(
            adopted.update_available(&["9999ffff-0000-0000-0000-000000000000".into()]),
            Some(true)
        );

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    #[test]
    fn a_directory_with_no_manifest_has_no_version() {
        assert_eq!(package_version(Path::new("/nonexistent/game")), None);
    }

    /// A record for a directory somebody deleted must not keep the title in the
    /// Installed list, nor offer an update for something that is not there.
    #[test]
    fn a_record_whose_directory_is_gone_is_stale() {
        let dir = std::env::temp_dir().join(format!("xgdk-stale-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        let mut present = record(&["content-a"]);
        present.dir = dir.clone();
        assert!(!present.is_stale());

        std::fs::remove_dir_all(&dir).expect("removes it");
        assert!(present.is_stale(), "the directory is gone");
    }

    /// Something updated the title outside the launcher, so the recorded content
    /// ids describe a build that is no longer on disk.
    #[test]
    fn a_tree_that_changed_underneath_the_record_is_detectable() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.matches_disk(Some("1.26.4501.0")), Some(true));
        assert_eq!(installed.matches_disk(Some("1.27.0.0")), Some(false));
        assert_eq!(
            installed.matches_disk(None),
            None,
            "nothing to compare is not a mismatch"
        );

        let mut versionless = installed.clone();
        versionless.package_version = None;
        assert_eq!(versionless.matches_disk(Some("1.26.4501.0")), None);
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
