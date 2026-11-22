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
//!
//! [`detect`] looks for any known anti-cheat's files in a game's folder, so a
//! local game can say what it ships before it is started.

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
    /// A directory a title using this anti-cheat ships, at its root or next
    /// to its executable, compared without case.
    pub title_dir: &'static str,
    /// Entry points that start this anti-cheat, lowercase; `*` matches any
    /// prefix.
    pub entry_points: &'static [&'static str],
}

pub const EAC: Runtime = Runtime {
    name: "eac_runtime",
    env: "PROTON_EAC_RUNTIME",
    label: "Easy Anti-Cheat runtime",
    url: "https://github.com/lutris/buildbot/releases/download/2022.06.23/eac_runtime-20240114.tar.xz",
    sha256: "8c711a3dd0906fdedd2fdcdedc7b33e09eff5aa22fe2b2dffcc046f02e194957",
    archive_dir: "eac_runtime",
    marker: "v2/lib64/easyanticheat_x64.dll",
    title_dir: "easyanticheat",
    entry_points: &["start_protected_game.exe", "easyanticheat_launcher.exe", "*_eac.exe"],
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
    title_dir: "battleye",
    entry_points: &["*_be.exe"],
};

pub const ALL: [Runtime; 2] = [EAC, BATTLEYE];

/// Ceiling for one archive. Both are well under it (27 KB and 9 MB).
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;

/// Records which archive a complete install came from, so a new pin replaces
/// it and a half-extracted one is never mistaken for complete.
const STAMP: &str = ".ferestre-sha256";

/// Whether the title installed at `game_dir`, started through `executable`
/// (a package-relative Windows path), uses `runtime`. Only these titles pay
/// for its download.
pub fn needed(runtime: &Runtime, game_dir: &Path, executable: &str) -> bool {
    let executable = executable.replace('\\', "/");
    let entry = executable
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let named = runtime
        .entry_points
        .iter()
        .any(|p| match p.strip_prefix('*') {
            Some(suffix) => entry.ends_with(suffix),
            None => entry == *p,
        });
    let beside = Path::new(&executable).parent().map(|p| game_dir.join(p));
    named
        || [Some(game_dir.to_path_buf()), beside]
            .into_iter()
            .flatten()
            .any(|dir| has_child_dir(&dir, runtime.title_dir))
}

fn has_child_dir(dir: &Path, name: &str) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_name().to_string_lossy().eq_ignore_ascii_case(name)
                && e.file_type().is_ok_and(|t| t.is_dir())
        })
    })
}

/// An anti-cheat whose files were found in a game's folder.
///
/// A file on disk is evidence, not proof: a build may ship a driver it never
/// starts (publicly modded builds keep it next to a Steam emulator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub name: &'static str,
    /// It ships a Windows kernel driver.
    pub kernel: bool,
    /// Proton has a bridge for it (one of [`ALL`]).
    pub proton_runtime: bool,
    /// The first file or folder found, as it is on disk.
    pub evidence: PathBuf,
}

/// The files and folders that give an anti-cheat away. Patterns are
/// lowercase, compared without case, with at most one `*`.
pub struct Signature {
    pub name: &'static str,
    /// It ships a Windows kernel driver.
    pub kernel: bool,
    /// Proton has a bridge for it (one of [`ALL`]).
    pub proton_runtime: bool,
    pub files: &'static [&'static str],
    pub dirs: &'static [&'static str],
}

pub const SIGNATURES: &[Signature] = &[
    Signature {
        name: "Easy Anti-Cheat",
        kernel: true,
        proton_runtime: true,
        files: &[
            "start_protected_game.exe",
            "easyanticheat_x64.dll",
            "easyanticheat_eos_setup.exe",
            "eaclaunch.exe",
        ],
        dirs: &["easyanticheat", "easyanticheat_eos"],
    },
    Signature {
        name: "BattlEye",
        kernel: true,
        proton_runtime: true,
        files: &["beservice*.exe", "beclient*.dll", "bedaisy.sys"],
        dirs: &["battleye"],
    },
    Signature {
        name: "Ricochet",
        kernel: true,
        proton_runtime: false,
        files: &["randgrid.sys"],
        dirs: &[],
    },
    Signature {
        name: "Riot Vanguard",
        kernel: true,
        proton_runtime: false,
        files: &["vgk.sys", "vgc.exe"],
        dirs: &[],
    },
    Signature {
        name: "EA AntiCheat",
        kernel: true,
        proton_runtime: false,
        files: &[
            "eaanticheat.gameservicelauncher.exe",
            "eaanticheat.gameservicelauncher.dll",
            "eaanticheat.installer.exe",
        ],
        dirs: &[],
    },
    Signature {
        name: "nProtect GameGuard",
        kernel: true,
        proton_runtime: false,
        files: &["npggsvc.exe", "gameguard.des"],
        dirs: &["gameguard"],
    },
    Signature {
        name: "ACE",
        kernel: true,
        proton_runtime: false,
        files: &["sguard64.exe", "ace-base*"],
        dirs: &["ace-base*"],
    },
    Signature {
        name: "XIGNCODE3",
        kernel: true,
        proton_runtime: false,
        files: &["x3.xem"],
        dirs: &["xigncode"],
    },
    Signature {
        name: "mhyprot",
        kernel: true,
        proton_runtime: false,
        files: &["mhyprot*.sys"],
        dirs: &[],
    },
];

/// Whether a lowercase `name` matches a lowercase `pattern` with at most one
/// `*`, which stands for any run of characters (`beservice*.exe`).
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            name.len() >= prefix.len() + suffix.len()
                && name.starts_with(prefix)
                && name.ends_with(suffix)
        }
        None => name == pattern,
    }
}

/// The anti-cheats whose files are in `game_dir` or beside `executable` (a
/// path relative to `game_dir`, `\` read as `/`, or absolute), one per
/// signature, in [`SIGNATURES`] order.
pub fn detect(game_dir: &Path, executable: &str) -> Vec<Detection> {
    let executable = executable.replace('\\', "/");
    let beside = Path::new(&executable).parent().map(|p| game_dir.join(p));
    let mut entries: Vec<(String, bool, PathBuf)> = Vec::new();
    let mut seen = Vec::new();
    for dir in [Some(game_dir.to_path_buf()), beside].into_iter().flatten() {
        if seen.contains(&dir) {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            seen.push(dir);
            continue;
        };
        let mut found: Vec<(String, bool, PathBuf)> = read
            .flatten()
            .map(|e| {
                let is_dir = e.path().is_dir();
                (
                    e.file_name().to_string_lossy().to_lowercase(),
                    is_dir,
                    e.path(),
                )
            })
            .collect();
        found.sort_by(|a, b| a.2.cmp(&b.2));
        entries.extend(found);
        seen.push(dir);
    }
    SIGNATURES
        .iter()
        .filter_map(|signature| {
            let evidence = entries.iter().find(|(name, is_dir, _)| {
                let patterns = if *is_dir {
                    signature.dirs
                } else {
                    signature.files
                };
                patterns.iter().any(|p| matches(p, name))
            })?;
            Some(Detection {
                name: signature.name,
                kernel: signature.kernel,
                proton_runtime: signature.proton_runtime,
                evidence: evidence.2.clone(),
            })
        })
        .collect()
}

/// One line for the CLI and the GUI saying which anti-cheat files were found.
/// It states evidence, not a verdict. `None` when nothing was found.
pub fn describe(found: &[Detection]) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let items: Vec<String> = found
        .iter()
        .map(|d| {
            let mut evidence = d
                .evidence
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| d.evidence.display().to_string());
            if d.evidence.is_dir() {
                evidence.push('/');
            }
            let mut notes = Vec::new();
            if d.kernel {
                notes.push("a kernel driver");
            }
            if d.proton_runtime {
                notes.push("Proton has a runtime for it");
            }
            if notes.is_empty() {
                format!("{} ({evidence})", d.name)
            } else {
                format!("{} ({evidence}, {})", d.name, notes.join("; "))
            }
        })
        .collect();
    Some(format!(
        "anti-cheat files found: {}. If this build uses them, play single-player offline only; \
         online play may be refused or lead to a ban.",
        items.join(", ")
    ))
}

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
    let path = root.join(runtime.name);
    // Left by launches that were killed part way.
    for extension in ["archive", "unpack", "old"] {
        crate::cache::sweep_stale_temporaries(&path, extension);
    }
    // Each launch has its own archive, so two first launches never share or
    // delete one another's.
    let archive = crate::cache::temporary_beside(&path, "archive");
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
        if let Some(done) = installed(root, runtime) {
            // Another launch finished while this one downloaded. Keep its
            // copy: a game it started may already be using it.
            return Ok(done);
        }
        if path.exists() {
            // An older pin or an incomplete copy. Move it aside rather than
            // deleting it in place, so the swap below is one rename.
            let old = crate::cache::temporary_beside(&path, "old");
            std::fs::rename(&path, &old)
                .with_context(|| format!("cannot replace {}", path.display()))?;
            let _ = std::fs::remove_dir_all(&old);
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

/// EAC's legacy Linux client library, for titles whose package has only the
/// Windows one (the Xbox PC build of Gears 5). Proton's EAC bridge loads it
/// from beside the game's `EasyAntiCheat_x64.dll`; the private image view
/// puts it there for the launch only. It is the same generic library that
/// legacy-EAC games ship for Linux; the game-specific part still comes from
/// Epic's servers.
pub struct Library {
    /// Where to fetch it. Empty until a download location is published, in
    /// which case only a file the user names (`FERESTRE_EAC_LINUX_LIBRARY`)
    /// is used.
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const EAC_LINUX_LIBRARY: Library = Library {
    url: "https://www.dropbox.com/scl/fi/wulm6yzv1ybt1nej11sdp/easyanticheat_x64.so?rlkey=vrt9hbjafnm3jux7qxrwl4p5z&dl=1",
    sha256: "109a015168b673b6d09c0d388bcfd46d46433e9723897e507d9431f5f2336209",
    size: 4_268_976,
};

/// The variable naming the library: a path the user chose, or `pinned` in a
/// recipe to ask for the downloaded one.
pub const LIBRARY_ENV: &str = "FERESTRE_EAC_LINUX_LIBRARY";

/// The recipe value asking for the pinned download.
pub const PINNED: &str = "pinned";

fn library_path(root: &Path) -> PathBuf {
    root.join("eac_linux").join("easyanticheat_x64.so")
}

/// The library's path if a verified copy is there.
pub fn library_installed(root: &Path, library: &Library) -> Option<PathBuf> {
    let path = library_path(root);
    crate::updater::verify_checksum(&path, library.sha256)
        .is_ok()
        .then_some(path)
}

/// The library's path, downloading it first if needed.
pub fn ensure_library(root: &Path, library: &Library) -> Result<PathBuf> {
    if let Some(path) = library_installed(root, library) {
        return Ok(path);
    }
    if library.url.is_empty() {
        bail!("no download location is configured; set {LIBRARY_ENV} to an easyanticheat_x64.so");
    }
    let path = library_path(root);
    crate::http::download_pinned(library.url, &path, library.sha256, library.size)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_titles_that_ship_an_anti_cheat_need_its_runtime() {
        let game = tempfile::tempdir().unwrap();
        let exe = "Game\\Binaries\\Win64\\Game.exe";
        assert!(!needed(&EAC, game.path(), exe));
        assert!(!needed(&BATTLEYE, game.path(), exe));
        std::fs::create_dir_all(game.path().join("Game/Binaries/Win64/EasyAntiCheat")).unwrap();
        assert!(needed(&EAC, game.path(), exe));
        assert!(!needed(&BATTLEYE, game.path(), exe));
        std::fs::create_dir(game.path().join("BattlEye")).unwrap();
        assert!(needed(&BATTLEYE, game.path(), exe));
    }

    #[test]
    fn an_anti_cheat_entry_point_names_its_runtime() {
        let game = tempfile::tempdir().unwrap();
        assert!(needed(
            &EAC,
            game.path(),
            "GearGame\\Binaries\\WinStore\\Gears5_EAC.exe"
        ));
        assert!(needed(&EAC, game.path(), "start_protected_game.exe"));
        assert!(needed(&BATTLEYE, game.path(), "Bin\\Game_BE.exe"));
        assert!(!needed(&EAC, game.path(), "Bin\\Game_BE.exe"));
    }

    #[test]
    fn a_library_is_installed_only_when_its_hash_matches() {
        let root = tempfile::tempdir().unwrap();
        let path = library_path(root.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"elf").unwrap();
        let other = Library {
            url: "",
            sha256: EAC_LINUX_LIBRARY.sha256,
            size: 3,
        };
        assert_eq!(library_installed(root.path(), &other), None);
        // sha256("elf")
        let matching = Library {
            sha256: "780d84b20d7ae7e6292919399348bdbf96025270136198083fc8a4da398b5ca9",
            ..other
        };
        assert_eq!(library_installed(root.path(), &matching), Some(path));
    }

    #[test]
    fn without_a_location_the_library_asks_for_a_file() {
        let root = tempfile::tempdir().unwrap();
        let none = Library {
            url: "",
            sha256: EAC_LINUX_LIBRARY.sha256,
            size: 1,
        };
        let e = ensure_library(root.path(), &none).unwrap_err();
        assert!(format!("{e:#}").contains(LIBRARY_ENV), "{e:#}");
    }

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

    #[test]
    fn a_kernel_anti_cheat_is_reported_once_as_evidence_even_where_the_build_is_modded() {
        // The CoD sp26 layout: the driver at the root and beside the exe, plus a Steam emulator.
        let game = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(game.path().join("sp26/steam_settings")).unwrap();
        for f in ["Randgrid.sys", "sp26/Randgrid.sys", "sp26/sp26-cod.exe"] {
            std::fs::write(game.path().join(f), b"").unwrap();
        }
        let found = detect(game.path(), "sp26/sp26-cod.exe");
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].name, found[0].kernel, found[0].proton_runtime),
            ("Ricochet", true, false)
        );
        let line = describe(&found).unwrap();
        assert!(
            line.contains("Randgrid.sys") && line.contains("If this build uses them"),
            "{line}"
        );
        assert!(
            !line.contains("will not run"),
            "evidence, not a verdict: {line}"
        );
    }

    #[test]
    fn every_signature_is_recognised_by_its_file_or_folder() {
        for (layout, name) in [
            ("EasyAntiCheat/", "Easy Anti-Cheat"),
            ("BEService_x64.exe", "BattlEye"),
            ("vgc.exe", "Riot Vanguard"),
            ("EAAntiCheat.GameServiceLauncher.exe", "EA AntiCheat"),
            ("GameGuard/", "nProtect GameGuard"),
            ("SGuard64.exe", "ACE"),
            ("ACE-Base64.sys", "ACE"),
            ("x3.xem", "XIGNCODE3"),
            ("mhyprot3.sys", "mhyprot"),
        ] {
            let game = tempfile::tempdir().unwrap();
            match layout.strip_suffix('/') {
                Some(dir) => std::fs::create_dir(game.path().join(dir)).unwrap(),
                None => std::fs::write(game.path().join(layout), b"").unwrap(),
            }
            let found = detect(game.path(), "Game.exe");
            assert_eq!(found.len(), 1, "{layout}");
            assert_eq!(found[0].name, name, "{layout}");
        }
        assert!(SIGNATURES.iter().all(|s| s.kernel));
        assert!(SIGNATURES
            .iter()
            .filter(|s| s.proton_runtime)
            .map(|s| s.name)
            .eq(["Easy Anti-Cheat", "BattlEye"]));
        assert!(detect(tempfile::tempdir().unwrap().path(), "Game.exe").is_empty());
        assert!(describe(&[]).is_none());
    }

    #[test]
    fn a_folder_is_named_as_a_folder_and_an_absolute_executable_is_honoured() {
        let game = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("EasyAntiCheat")).unwrap();
        let exe = other.path().join("Launcher.exe");
        let found = detect(game.path(), exe.to_str().unwrap());
        assert_eq!(found.len(), 1);
        let line = describe(&found).unwrap();
        assert!(
            line.contains("EasyAntiCheat/") && line.contains("Proton has a runtime for it"),
            "{line}"
        );
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
        // Nothing but the runtimes is left behind.
        let mut names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["battleye_runtime", "eac_runtime"]);
    }

    #[test]
    #[ignore = "downloads from dropbox.com"]
    fn a_download_that_does_not_match_its_pin_is_rejected_and_not_kept() {
        let root = tempfile::tempdir().unwrap();
        let wrong = Library {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..EAC_LINUX_LIBRARY
        };
        let e = ensure_library(root.path(), &wrong).unwrap_err();
        assert!(
            format!("{e:#}").contains("does not match its pinned SHA-256"),
            "{e:#}"
        );
        let dir = root.path().join("eac_linux");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .map(|d| d.flatten().collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    #[ignore = "downloads from dropbox.com"]
    fn the_pinned_eac_linux_library_downloads() {
        let root = tempfile::tempdir().unwrap();
        let path = ensure_library(root.path(), &EAC_LINUX_LIBRARY).unwrap();
        assert_eq!(std::fs::read(&path).unwrap()[..4], *b"\x7fELF");
        assert_eq!(
            library_installed(root.path(), &EAC_LINUX_LIBRARY),
            Some(path)
        );
    }
}
