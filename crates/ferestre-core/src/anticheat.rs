//! Valve's Proton anti-cheat runtimes, fetched the way Lutris and Heroic do.
//!
//! Easy Anti-Cheat and BattlEye each have a Wine bridge that Steam installs as
//! a separate tool ("Proton EasyAntiCheat Runtime", "Proton BattlEye
//! Runtime"). Proton's Wine finds it through `PROTON_EAC_RUNTIME` and
//! `PROTON_BATTLEYE_RUNTIME`. Without Steam's copy, Lutris publishes the same
//! files on GitHub, and Lutris and Heroic both download those. So does this,
//! from the same URLs, but only keeps an archive whose SHA-256 is the one
//! pinned here. The pinned EAC archive holds the same bytes as Steam's app
//! 1826330.
//!
//! The EAC bridge only takes over when a title ships EAC's Linux library
//! (`easyanticheat_x64.so`) next to its Windows one; Wine's load order checks
//! for it. A title without it loads its own Windows EAC library exactly as
//! before, so setting the variable is safe for every title.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One runtime: where Lutris publishes it and what Proton calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    /// Directory name under [`dir`], as Lutris and Heroic name it.
    pub name: &'static str,
    /// The variable Proton's Wine reads.
    pub env: &'static str,
    /// Shown in progress and warning lines.
    pub label: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// The archive's top-level directory, which is not always `name`.
    pub archive_dir: &'static str,
    /// A file every complete copy has, relative to the runtime directory.
    pub marker: &'static str,
}

pub const EAC: Runtime = Runtime {
    name: "eac_runtime",
    env: "PROTON_EAC_RUNTIME",
    label: "Easy Anti-Cheat runtime",
    url: "https://github.com/lutris/buildbot/releases/download/2022.06.23/eac_runtime-20240114.tar.xz",
    sha256: "8c711a3dd0906fdedd2fdcdedc7b33e09eff5aa22fe2b2dffcc046f02e194957",
    archive_dir: "eac_runtime",
    marker: "v2/lib64/easyanticheat_x64.dll",
};

pub const BATTLEYE: Runtime = Runtime {
    name: "battleye_runtime",
    env: "PROTON_BATTLEYE_RUNTIME",
    label: "BattlEye runtime",
    url: "https://github.com/lutris/buildbot/releases/download/2024-05-21/battleeye_runtime.tar.xz",
    sha256: "229eaedf3bc16d5bf548bc2aed94bc48c44e2038337eedacbd17c6c7244cd212",
    // Lutris's archive spells it this way.
    archive_dir: "battleeye_runtime",
    marker: "v1/lib/wine/beclient.dll.so",
};

pub const ALL: [Runtime; 2] = [EAC, BATTLEYE];

/// Ceiling for one archive. Both are well under it (27 KB and 9 MB).
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;

/// Records which archive a complete install came from, so a new pin replaces
/// it and a half-extracted one is never mistaken for complete.
const STAMP: &str = ".ferestre-sha256";

/// Where the runtimes live, under the launcher's state directory.
pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("anticheat")
}

/// The runtime's directory if a complete copy of the pinned archive is there.
pub fn installed(root: &Path, runtime: &Runtime) -> Option<PathBuf> {
    let path = root.join(runtime.name);
    let stamp = std::fs::read_to_string(path.join(STAMP)).ok()?;
    (stamp.trim() == runtime.sha256 && path.join(runtime.marker).is_file()).then_some(path)
}

/// The runtime's directory, downloading and unpacking it first if needed.
pub fn ensure(root: &Path, runtime: &Runtime) -> Result<PathBuf> {
    if let Some(path) = installed(root, runtime) {
        return Ok(path);
    }
    std::fs::create_dir_all(root).with_context(|| format!("{}", root.display()))?;
    let archive = root.join(format!(".{}.tar.xz", runtime.name));
    crate::http::download_pinned(runtime.url, &archive, runtime.sha256, MAX_ARCHIVE)?;
    let result = unpack(root, runtime, &archive);
    let _ = std::fs::remove_file(&archive);
    result
}

fn unpack(root: &Path, runtime: &Runtime, archive: &Path) -> Result<PathBuf> {
    // Unpacked beside its final place and renamed in, so a launch running at
    // the same time sees either no runtime or a whole one.
    let staging = crate::cache::temporary_beside(&root.join(runtime.name), "unpack");
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> Result<PathBuf> {
        let status = Command::new("tar")
            .arg("-xJf")
            .arg(archive)
            .arg("-C")
            .arg(&staging)
            .args(["--no-same-owner", "--no-same-permissions"])
            .status()
            .context("cannot run tar to unpack the runtime")?;
        if !status.success() {
            bail!("tar could not unpack {} ({status})", archive.display());
        }
        let unpacked = staging.join(runtime.archive_dir);
        if !unpacked.join(runtime.marker).is_file() {
            bail!("the archive has no {}", runtime.marker);
        }
        std::fs::write(unpacked.join(STAMP), runtime.sha256)?;
        let path = root.join(runtime.name);
        if path.exists() {
            // An older pin or an incomplete copy: `installed` said no.
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("cannot replace {}", path.display()))?;
        }
        if let Err(e) = std::fs::rename(&unpacked, &path) {
            // Another launch finished first.
            if installed(root, runtime).is_none() {
                return Err(e).with_context(|| format!("{}", path.display()));
            }
        }
        Ok(path)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(root: &Path, runtime: &Runtime, stamp: &str) -> PathBuf {
        let path = root.join(runtime.name);
        let marker = path.join(runtime.marker);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, b"x").unwrap();
        std::fs::write(path.join(STAMP), stamp).unwrap();
        path
    }

    #[test]
    fn a_complete_copy_of_the_pinned_archive_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let path = fake(root.path(), &EAC, EAC.sha256);
        assert_eq!(installed(root.path(), &EAC), Some(path));
        assert_eq!(installed(root.path(), &BATTLEYE), None);
    }

    #[test]
    fn a_copy_from_another_archive_is_not() {
        let root = tempfile::tempdir().unwrap();
        fake(root.path(), &EAC, "0000");
        assert_eq!(installed(root.path(), &EAC), None);
    }

    #[test]
    fn a_copy_missing_its_bridge_is_not() {
        let root = tempfile::tempdir().unwrap();
        let path = fake(root.path(), &EAC, EAC.sha256);
        std::fs::remove_file(path.join(EAC.marker)).unwrap();
        assert_eq!(installed(root.path(), &EAC), None);
    }

    #[test]
    fn ensure_unpacks_an_archive_already_downloaded() {
        // Build an archive shaped like Lutris's, pin-free: `unpack` is what
        // runs after the download has been checked.
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let marker = src.path().join(EAC.archive_dir).join(EAC.marker);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, b"bridge").unwrap();
        let archive = root.path().join("a.tar.xz");
        let ok = Command::new("tar")
            .arg("-cJf")
            .arg(&archive)
            .arg("-C")
            .arg(src.path())
            .arg(EAC.archive_dir)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let path = unpack(root.path(), &EAC, &archive).unwrap();
        assert_eq!(path, root.path().join("eac_runtime"));
        assert_eq!(installed(root.path(), &EAC), Some(path));
        // Nothing left over from staging.
        let leftovers: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("unpack"))
            .collect();
        assert!(leftovers.is_empty());
    }
}

/// Fetches both archives from GitHub: `cargo test -- --ignored anticheat`.
#[cfg(test)]
mod network {
    use super::*;

    #[test]
    #[ignore = "downloads from github.com"]
    fn both_pinned_archives_download_and_unpack() {
        let root = tempfile::tempdir().unwrap();
        for runtime in &ALL {
            let path = ensure(root.path(), runtime).unwrap();
            assert!(path.join(runtime.marker).is_file(), "{}", runtime.name);
            assert_eq!(installed(root.path(), runtime), Some(path));
        }
        // The archive is not kept once unpacked.
        assert!(!root.path().join(".eac_runtime.tar.xz").exists());
    }
}
