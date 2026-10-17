//! Complete cache snapshots survive closing during a refresh.
use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

pub const LIBRARY_MAX_AGE: Duration = Duration::from_secs(5 * 60);
pub const CATALOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const GAMEPASS_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Missing, unreadable and future-dated entries need a check. Existing data
/// remains available while the caller refreshes it in the background.
pub fn is_fresh(path: &Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < max_age)
}

/// Publish by rename, never by truncating the last good snapshot. A process
/// exit during a refresh can leave a temporary file, which readers ignore.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_keeps_a_complete_snapshot_and_missing_caches_are_stale() {
        let dir = std::env::temp_dir().join(format!("ferestre-cache-{}", std::process::id()));
        let path = dir.join("listing.json");
        assert!(!is_fresh(&path, LIBRARY_MAX_AGE));
        write(&path, b"old").unwrap();
        write(&path, b"complete replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"complete replacement");
        assert!(is_fresh(&path, LIBRARY_MAX_AGE));
        assert!(!is_fresh(&path, Duration::ZERO));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
