//! Moving a recorded installation. The title's lock is held by the caller.
//! Cross-device moves publish only a complete, read-back verified copy. Prefixes
//! and saves remain in their existing location; launch resolution uses the record.
use crate::{human_bytes, install};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Plan {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub bytes: u64,
    pub available: u64,
    pub same_filesystem: bool,
    entries: Vec<(PathBuf, bool, u64)>,
}

pub fn available_space(path: &Path) -> Result<u64> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // Safety: the C string and output buffer live through this call.
    ensure!(
        unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0,
        "Cannot check destination space: {}",
        std::io::Error::last_os_error()
    );
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// The argument is an existing library folder; the game's own folder is added.
/// This deliberately refuses merging with an existing directory.
pub fn plan(record: &install::Record, library: &Path) -> Result<Plan> {
    ensure!(
        !record.is_stale() && !install::is_partial(&record.dir),
        "Finish installing this title before moving it"
    );
    install::safe_to_remove(&record.dir)?;
    let source = record.dir.canonicalize()?;
    let library = library
        .canonicalize()
        .context("Choose an existing destination folder")?;
    ensure!(library.is_dir(), "The destination must be a folder");
    let destination = library.join(source.file_name().context("Missing game folder name")?);
    ensure!(
        destination != source,
        "The game is already in this location"
    );
    ensure!(
        !destination.starts_with(&source) && !source.starts_with(&destination),
        "Choose a folder outside the current installation"
    );
    ensure!(
        !destination.try_exists()? && !destination.is_symlink(),
        "{} already exists; choose another folder",
        destination.display()
    );
    let same_filesystem = fs::metadata(&source)?.dev() == fs::metadata(&library)?.dev();
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    walk(&source, Path::new(""), &mut entries, &mut bytes)?;
    entries.sort();
    let available = available_space(&library)?;
    // A little headroom for directory entries, allocation rounding and other
    // applications writing to the destination while the move is in progress.
    if !same_filesystem {
        let required = bytes
            .saturating_add(64 * 1024 * 1024)
            .saturating_add(entries.len() as u64 * 4096);
        ensure!(
            available >= required,
            "Not enough space: {} needed, {} available",
            human_bytes(required),
            human_bytes(available)
        );
    }
    Ok(Plan {
        source,
        destination,
        bytes,
        available,
        same_filesystem,
        entries,
    })
}

/// Returns a cleanup warning if the new install is committed but deleting the
/// old copy failed. A cancelled/failed copy leaves the original record and data.
pub fn execute(
    state: &Path,
    record: &install::Record,
    plan: &Plan,
    mut check: impl FnMut() -> Result<()>,
    mut progress: impl FnMut(u64, u64),
) -> Result<Option<String>> {
    check()?;
    ensure!(
        install::load(state, &record.product_id).as_ref() == Some(record),
        "Installation changed; try again"
    );
    ensure!(
        !plan.destination.try_exists()? && !plan.destination.is_symlink(),
        "Destination already exists"
    );
    let mut moved = record.clone();
    moved.dir = plan.destination.clone();
    let parent = plan.destination.parent().unwrap();
    // create_dir reserves a name atomically and refuses abandoned previous
    // work. Never remove anything we did not create in this invocation.
    let staging = parent.join(format!(
        ".ferestre-move-{}-{}",
        record.product_id,
        std::process::id()
    ));
    fs::create_dir(&staging).context("Creating temporary destination")?;
    let total = plan.bytes.saturating_mul(2);
    let mut done = 0;
    progress(0, total);
    let copy = (|| -> Result<()> {
        let mut buffer = vec![0u8; 1024 * 1024];
        for (relative, directory, size) in &plan.entries {
            check()?;
            let source = plan.source.join(relative);
            let target = staging.join(relative);
            let before = fs::symlink_metadata(&source)?;
            ensure!(
                !before.file_type().is_symlink(),
                "Source changed during move"
            );
            if *directory {
                fs::create_dir(&target)?;
                continue;
            }
            ensure!(
                before.is_file() && before.len() == *size,
                "Source changed during move"
            );
            if plan.same_filesystem {
                // Hard links stage the same bytes without duplicating disk
                // space. A crash before the record is saved still leaves the
                // entire original installation available.
                fs::hard_link(&source, &target)?;
                done += size.saturating_mul(2);
                progress(done, total);
                continue;
            }
            let mut input = File::open(&source)?;
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)?;
            let mut expected = Sha256::new();
            let mut copied = 0;
            loop {
                check()?;
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                output.write_all(&buffer[..n])?;
                expected.update(&buffer[..n]);
                copied += n as u64;
                done += n as u64;
                progress(done, total);
            }
            ensure!(copied == *size, "Source changed during move");
            output.sync_all()?;
            fs::set_permissions(&target, before.permissions())?;
            drop(output);
            let mut verify = File::open(&target)?;
            let mut actual = Sha256::new();
            loop {
                check()?;
                let n = verify.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                actual.update(&buffer[..n]);
                done += n as u64;
                progress(done, total);
            }
            ensure!(
                expected.finalize() == actual.finalize(),
                "Verification failed for {}",
                relative.display()
            );
            let after = fs::metadata(&source)?;
            ensure!(
                after.len() == before.len() && after.modified()? == before.modified()?,
                "Source changed during move"
            );
        }
        // Detect additions/removals as well as changes to copied files.
        let mut entries = Vec::new();
        let mut bytes = 0;
        walk(&plan.source, Path::new(""), &mut entries, &mut bytes)?;
        entries.sort();
        ensure!(
            entries == plan.entries,
            "Installation changed during move; try again"
        );
        // Persist every directory entry before publishing the tree. Syncing
        // only the root would leave nested names vulnerable to a power loss
        // after the install record is committed and the source is removed.
        for (relative, directory, _) in plan.entries.iter().rev() {
            if *directory {
                let target = staging.join(relative);
                fs::set_permissions(
                    &target,
                    fs::metadata(plan.source.join(relative))?.permissions(),
                )?;
                File::open(&target)?.sync_all()?;
            }
        }
        fs::set_permissions(&staging, fs::metadata(&plan.source)?.permissions())?;
        File::open(&staging)?.sync_all()?;
        check()?;
        Ok(())
    })();
    if let Err(error) = copy {
        let cleanup = fs::remove_dir_all(&staging);
        return Err(error).with_context(|| match cleanup {
            Ok(()) => "Move stopped; original installation kept".to_string(),
            Err(e) => format!(
                "Original installation kept; remove {} manually ({e})",
                staging.display()
            ),
        });
    }
    if let Err(error) = publish(&staging, &plan.destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error).context("Publishing the verified destination");
    }
    File::open(parent)?.sync_all()?;
    if let Err(error) = install::save(state, &moved) {
        let _ = fs::remove_dir_all(&plan.destination);
        return Err(error).context("Original installation kept; could not save its new location");
    }
    if let Err(error) = fs::remove_dir_all(&plan.source) {
        return Ok(Some(format!(
            "Moved successfully; old files remain at {}: {error}",
            plan.source.display()
        )));
    }
    Ok(None)
}

fn walk(
    root: &Path,
    relative: &Path,
    entries: &mut Vec<(PathBuf, bool, u64)>,
    bytes: &mut u64,
) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let meta = fs::symlink_metadata(entry.path())?;
        // Following a link could copy a different game or data outside the
        // install. Reject special files before changing anything.
        ensure!(
            meta.is_file() || meta.is_dir(),
            "{} contains a link or special file; move it manually",
            entry.path().display()
        );
        entries.push((path.clone(), meta.is_dir(), meta.len()));
        if meta.is_dir() {
            walk(root, &path, entries, bytes)?;
        } else {
            *bytes = bytes
                .checked_add(meta.len())
                .context("Installation is too large")?;
        }
    }
    Ok(())
}

fn publish(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source = std::ffi::CString::new(source.as_os_str().as_encoded_bytes())?;
    let destination = std::ffi::CString::new(destination.as_os_str().as_encoded_bytes())?;
    // Safety: both C strings live through the call. NOREPLACE also protects
    // against another process creating the destination after the plan check.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir, TempDir};
    fn fixture() -> (TempDir, install::Record) {
        let root = tempdir().unwrap();
        let source = root.path().join("game");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("MicrosoftGame.config"),
            "<Game><Identity Name=\"Fixture\" Version=\"1.0.0.0\"/></Game>",
        )
        .unwrap();
        fs::write(source.join(".xodus-streaming.msixvc"), b"fixture header").unwrap();
        fs::write(source.join("payload.bin"), vec![42; 2 * 1024 * 1024]).unwrap();
        let record = install::Record {
            product_id: "9TESTMOVETTL".into(),
            dir: source,
            content_ids: vec!["fixture".into()],
            installed_at: None,
            package_version: Some("1.0.0.0".into()),
        };
        install::save(root.path(), &record).unwrap();
        (root, record)
    }
    #[test]
    fn same_filesystem_publishes_before_removing_source_and_preserves_record() {
        let (root, record) = fixture();
        let library = root.path().join("library");
        fs::create_dir(&library).unwrap();
        let plan = plan(&record, &library).unwrap();
        assert!(plan.same_filesystem);
        let mut progress_seen = false;
        execute(
            root.path(),
            &record,
            &plan,
            || Ok(()),
            |done, total| {
                progress_seen |= done > 0 && total > 0;
            },
        )
        .unwrap();
        assert!(progress_seen);
        assert!(!record.dir.exists());
        let moved = install::load(root.path(), &record.product_id).unwrap();
        assert_eq!(moved.dir, library.join("game"));
        assert_eq!(moved.content_ids, record.content_ids);
        assert_eq!(
            fs::read(moved.dir.join("payload.bin")).unwrap(),
            vec![42; 2 * 1024 * 1024]
        );
    }
    #[test]
    fn cross_filesystem_copy_is_verified_and_cancellation_keeps_source() {
        let (root, record) = fixture();
        let library = tempfile::tempdir_in("/dev/shm").unwrap();
        let plan = plan(&record, library.path()).unwrap();
        assert!(!plan.same_filesystem);
        let mut checks = 0;
        let error = execute(
            root.path(),
            &record,
            &plan,
            || {
                checks += 1;
                ensure!(checks < 5, "cancelled");
                Ok(())
            },
            |_, _| {},
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("cancelled"));
        assert_eq!(
            install::load(root.path(), &record.product_id),
            Some(record.clone())
        );
        assert!(record.dir.exists());
        assert_eq!(fs::read_dir(library.path()).unwrap().count(), 0);
        execute(root.path(), &record, &plan, || Ok(()), |_, _| {}).unwrap();
        assert!(!record.dir.exists());
        assert_eq!(
            fs::read(plan.destination.join("payload.bin")).unwrap(),
            vec![42; 2 * 1024 * 1024]
        );
    }
    #[test]
    fn destination_collision_nested_path_and_symlinks_are_refused() {
        let (root, record) = fixture();
        assert!(plan(&record, root.path()).is_err());
        let inside = record.dir.join("nested");
        fs::create_dir(&inside).unwrap();
        assert!(plan(&record, &inside).is_err());
        fs::remove_dir(&inside).unwrap();
        let library = root.path().join("library");
        fs::create_dir(&library).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", record.dir.join("link")).unwrap();
        assert!(plan(&record, &library).is_err());
        assert!(record.dir.exists());
    }
    #[test]
    fn insufficient_space_is_refused_before_copying() {
        let (root, record) = fixture();
        let library = tempfile::tempdir_in("/dev/shm").unwrap();
        let huge = File::create(record.dir.join("huge")).unwrap();
        huge.set_len(available_space(library.path()).unwrap() + 1)
            .unwrap();
        let error = plan(&record, library.path()).unwrap_err();
        assert!(error.to_string().contains("Not enough space"));
        assert!(record.dir.exists());
        assert_eq!(fs::read_dir(library.path()).unwrap().count(), 0);
        drop(root);
    }
}
