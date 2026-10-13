//! Recovery for the older native SDK's cache of never-uploaded files.
//! The game files remain intact. Removing only a wholly unsynced cache makes
//! the SDK rediscover those files instead of promoting them to cloud deletions.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn wholly_unsynced_legacy_cache(value: &Value) -> bool {
    let Some(folders) = value.get("Folders").and_then(Value::as_array) else {
        return false;
    };
    let mut files = 0;
    for folder in folders {
        let Some(entries) = folder.get("Files").and_then(Value::as_array) else {
            return false;
        };
        for entry in entries {
            files += 1;
            if entry.get("hasLastSync").is_some()
                || entry.get("lastSyncFileSize").and_then(Value::as_u64) != Some(0)
                || !matches!(
                    entry
                        .get("lastSyncTimeLastModified")
                        .and_then(Value::as_str),
                    Some("1970-01-01T00:00:00.000Z" | "1970-01-01T00:00:00Z")
                )
            {
                return false;
            }
        }
    }
    files != 0
}

/// Back up and retire a cache that contains no evidence of a completed sync.
/// A real sync baseline, an explicit modern marker, or unknown data is retained.
pub fn repair_unsynced_cache(data: &Path, backup_root: &Path) -> Result<Option<PathBuf>> {
    let cache = data.join("cloudsync/localstate.json");
    let bytes = match std::fs::read(&cache) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading PlayFab save synchronization cache"),
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    if !wholly_unsynced_legacy_cache(&value) {
        return Ok(None);
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = backup_root.join(format!("unsynced-cache-{timestamp}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&directory)?;
    let backup = directory.join("localstate.json");
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&backup)?;
    use std::io::Write;
    file.write_all(&bytes)?;
    file.sync_all()?;
    anyhow::ensure!(
        std::fs::read(&backup)? == bytes,
        "save cache backup verification failed"
    );
    // Don't remove a cache a running writer changed while it was being backed up.
    anyhow::ensure!(
        std::fs::read(&cache)? == bytes,
        "save cache changed during recovery"
    );
    std::fs::remove_file(cache)?;
    Ok(Some(backup))
}

/// A prefix can contain multiple package families. Only inspect this package.
pub fn repair_package(prefix: &Path, identity: &str, backups: &Path) -> Result<Vec<PathBuf>> {
    let packages = prefix.join("pfx/drive_c/users/steamuser/AppData/Local/Packages");
    let entries = match std::fs::read_dir(packages) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("reading package save directories"),
    };
    let family_prefix = format!("{identity}_");
    let mut repaired = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|n| n.strip_prefix(&family_prefix)) else {
            continue;
        };
        if suffix.len() != 13
            || !suffix.bytes().all(|c| c.is_ascii_alphanumeric())
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let data = entry.path().join("SystemAppData/PFXGameSave/data");
        if let Some(backup) = repair_unsynced_cache(&data, backups)? {
            repaired.push(backup);
        }
    }
    Ok(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn legacy(size: u64, time: &str) -> Value {
        json!({"Folders":[{"Files":[{"fileName":"SaveVersion","lastSyncFileSize":size,"lastSyncTimeLastModified":time}]}]})
    }

    #[test]
    fn only_the_never_synced_legacy_state_is_repaired() {
        assert!(wholly_unsynced_legacy_cache(&legacy(
            0,
            "1970-01-01T00:00:00.000Z"
        )));
        assert!(!wholly_unsynced_legacy_cache(&legacy(
            40,
            "1970-01-01T00:00:00.000Z"
        )));
        assert!(!wholly_unsynced_legacy_cache(&legacy(
            0,
            "2026-10-13T12:00:00.000Z"
        )));
        let mut modern = legacy(0, "1970-01-01T00:00:00.000Z");
        modern["Folders"][0]["Files"][0]["hasLastSync"] = json!(false);
        assert!(!wholly_unsynced_legacy_cache(&modern));
        assert!(!wholly_unsynced_legacy_cache(&json!({"Folders":[]})));
        assert!(!wholly_unsynced_legacy_cache(&json!({"Folders":[{}]})));
    }

    #[test]
    fn other_package_families_and_completed_sync_baselines_are_preserved() {
        let root =
            std::env::temp_dir().join(format!("ferestre-playfab-scoped-{}", std::process::id()));
        let base = root.join("prefix/pfx/drive_c/users/steamuser/AppData/Local/Packages");
        let data = base.join("Fixture_8wekyb3d8bbwe/SystemAppData/PFXGameSave/data");
        let other = base.join("Other_8wekyb3d8bbwe/SystemAppData/PFXGameSave/data");
        let cache = legacy(0, "1970-01-01T00:00:00.000Z").to_string();
        for directory in [&data, &other] {
            std::fs::create_dir_all(directory.join("cloudsync")).unwrap();
            std::fs::write(directory.join("cloudsync/localstate.json"), &cache).unwrap();
        }
        let backups = root.join("backups");
        assert_eq!(
            repair_package(&root.join("prefix"), "Fixture", &backups)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(other.join("cloudsync/localstate.json")).unwrap(),
            cache
        );
        let synced = legacy(40, "2026-10-13T12:00:00.000Z").to_string();
        std::fs::write(data.join("cloudsync/localstate.json"), &synced).unwrap();
        assert!(repair_package(&root.join("prefix"), "Fixture", &backups)
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(data.join("cloudsync/localstate.json")).unwrap(),
            synced
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_keeps_game_files_device_identity_and_exact_cache_backup() {
        let root = std::env::temp_dir().join(format!("ferestre-playfab-{}", std::process::id()));
        let data = root.join("data");
        std::fs::create_dir_all(data.join("cloudsync")).unwrap();
        let bytes = serde_json::to_vec(&legacy(0, "1970-01-01T00:00:00.000Z")).unwrap();
        std::fs::write(data.join("cloudsync/localstate.json"), &bytes).unwrap();
        std::fs::write(data.join("cloudsync/info.json"), "device identity").unwrap();
        std::fs::write(data.join("profile"), "player progress").unwrap();
        let backup = repair_unsynced_cache(&data, &root.join("backups"))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        assert!(!data.join("cloudsync/localstate.json").exists());
        assert_eq!(
            std::fs::read_to_string(data.join("cloudsync/info.json")).unwrap(),
            "device identity"
        );
        assert_eq!(
            std::fs::read_to_string(data.join("profile")).unwrap(),
            "player progress"
        );
        assert!(repair_unsynced_cache(&data, &root.join("backups"))
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
