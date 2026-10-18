//! Complete cache snapshots survive closing during a refresh.
use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

pub const LIBRARY_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const UPDATES_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub const DISK_USAGE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
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

/// Temporary files older than this belong to a process that is gone. Nothing
/// writes one for anywhere near this long, so removing them cannot race a
/// writer that is still working.
const STALE_TEMPORARY: Duration = Duration::from_secs(24 * 60 * 60);

/// Publish by rename, never by truncating the last good snapshot. A process
/// exit during a refresh can leave a temporary file, which readers ignore and
/// a later write of the same file sweeps away.
///
/// The data is flushed before the rename and the directory after it. Without
/// the first, a power cut shortly after a refresh can leave the renamed file
/// empty on XFS, or on ext4 mounted without `auto_da_alloc`, which is exactly
/// the last good snapshot this is meant to keep. `account-roles.json` goes
/// through here too, and an empty one of those is not a cache that refreshes
/// itself.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    write_mode(path, bytes, 0o600)
}

/// [`write`] with the permissions a new file is created with, for files that
/// are not ours alone to read, such as Steam's `shortcuts.vdf`. The process
/// umask still applies.
pub(crate) fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    sweep_stale_temporaries(path, "tmp");
    let temporary = temporary_beside(path, "tmp");
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        sync_directory(parent);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// A name beside `path` that no other writer, in this process or another, will
/// pick at the same time. Callers create it with `create_new`, so even a
/// collision would fail rather than truncate someone else's file.
pub(crate) fn temporary_beside(path: &Path, extension: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(
        ".{}.{}-{}.{extension}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Remove temporaries of `path` left behind by a process that was killed.
/// Best effort: a sweep that fails costs disk space, never the write.
pub(crate) fn sweep_stale_temporaries(path: &Path, extension: &str) {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        ".{}.",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let suffix = format!(".{extension}");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(&suffix))
        else {
            continue;
        };
        // Only the shape temporary_beside makes: <pid>-<counter>.
        let ours = middle
            .split_once('-')
            .is_some_and(|(pid, n)| is_digits(pid) && is_digits(n));
        if ours && !is_fresh(&entry.path(), STALE_TEMPORARY) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Make a rename in `dir` durable. Best effort: some filesystems refuse to
/// sync a directory, and the file itself is already on disk by then.
pub(crate) fn sync_directory(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
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

    /// A process killed between creating its temporary file and renaming it
    /// leaves the file behind. The next write of the same file clears it once
    /// it is old enough that no live writer can still own it.
    #[test]
    fn temporaries_left_by_a_killed_writer_are_swept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("listing.json");
        let stale = dir.path().join(".listing.json.4242-0.tmp");
        let recent = dir.path().join(".listing.json.4243-0.tmp");
        let unrelated = dir.path().join(".other.json.4242-0.tmp");
        let not_ours = dir.path().join(".listing.json.keep.tmp");
        for file in [&stale, &recent, &unrelated, &not_ours] {
            std::fs::write(file, b"partial").unwrap();
        }
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        for file in [&stale, &unrelated, &not_ours] {
            std::fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        write(&path, b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        assert!(!stale.exists(), "an abandoned temporary is removed");
        assert!(
            recent.exists(),
            "a writer that may still be running is left alone"
        );
        assert!(
            unrelated.exists(),
            "another file's temporaries are not ours"
        );
        assert!(
            not_ours.exists(),
            "only the names this module makes are swept"
        );
    }

    #[test]
    fn a_new_file_is_private_unless_asked_otherwise() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("account-roles.json");
        write(&private, b"{}").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&private), 0o600);
        let shared = dir.path().join("shortcuts.vdf");
        write_mode(&shared, b"x", 0o644).unwrap();
        assert_eq!(mode(&shared) & 0o600, 0o600);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
