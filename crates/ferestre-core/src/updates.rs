//! Persist game-update checks so startup and navigation reuse the last result.
use crate::install::Record;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Entry {
    schema: u32,
    record: Record,
    available: Option<String>,
}

pub fn cache_path(state_dir: &Path, product_id: &str) -> Option<PathBuf> {
    let name = crate::install::record_path(state_dir, product_id)?
        .file_name()?
        .to_owned();
    Some(state_dir.join("updates").join(name))
}

fn load(state_dir: &Path, record: &Record) -> Option<Entry> {
    let path = cache_path(state_dir, &record.product_id)?;
    let entry: Entry = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (entry.schema == 1 && entry.record == *record).then_some(entry)
}

pub fn is_fresh(state_dir: &Path, record: &Record) -> bool {
    load(state_dir, record).is_some()
        && cache_path(state_dir, &record.product_id)
            .is_some_and(|path| crate::cache::is_fresh(&path, crate::cache::UPDATES_MAX_AGE))
}

/// Expired versions remain usable while a background check runs. Changed
/// installs cannot reuse the old version and accidentally offer a downgrade.
pub fn available<'a>(
    state_dir: &Path,
    records: impl IntoIterator<Item = &'a Record>,
) -> BTreeMap<String, String> {
    records
        .into_iter()
        .filter_map(|record| {
            Some((
                record.product_id.to_ascii_uppercase(),
                load(state_dir, record)?.available?,
            ))
        })
        .collect()
}

/// Record the attempt even on failure, keeping the last good version. Repeated
/// offline startups should not retry expensive checks; Refresh bypasses the age.
pub fn save(state_dir: &Path, record: &Record, version: Option<String>) -> Result<Option<String>> {
    let available = version.or_else(|| load(state_dir, record).and_then(|entry| entry.available));
    let entry = Entry {
        schema: 1,
        record: record.clone(),
        available: available.clone(),
    };
    let path = cache_path(state_dir, &record.product_id)
        .ok_or_else(|| anyhow::anyhow!("not a usable product id: {:?}", record.product_id))?;
    crate::cache::write(&path, &serde_json::to_vec(&entry)?)?;
    Ok(available)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn checks_survive_restart_expire_daily_and_keep_results_on_failure() {
        let dir = std::env::temp_dir().join(format!("ferestre-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut record = Record {
            product_id: "9ZZTESTUPDATE".into(),
            dir: "/games/test".into(),
            content_ids: vec!["content".into()],
            installed_at: None,
            package_version: Some("1".into()),
        };
        assert!(!is_fresh(&dir, &record));
        save(&dir, &record, Some("2".into())).unwrap();
        let path = cache_path(&dir, &record.product_id).unwrap();
        // A six-hour-old check is still reusable, unlike the old per-view queries.
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::now() - Duration::from_secs(6 * 3600)),
        )
        .unwrap();
        assert!(is_fresh(&dir, &record));
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::now() - Duration::from_secs(25 * 3600)),
        )
        .unwrap();
        assert!(!is_fresh(&dir, &record));
        assert_eq!(
            available(&dir, [&record])
                .get(&record.product_id)
                .map(String::as_str),
            Some("2")
        );
        assert_eq!(save(&dir, &record, None).unwrap().as_deref(), Some("2"));
        assert!(
            is_fresh(&dir, &record),
            "a failed attempt is not retried every startup"
        );
        record.package_version = Some("3".into());
        assert!(!is_fresh(&dir, &record));
        assert!(
            available(&dir, [&record]).is_empty(),
            "an old result cannot offer a downgrade after an install changes"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
