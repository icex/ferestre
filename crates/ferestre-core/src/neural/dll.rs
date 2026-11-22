//! Finding the person's own `nvngx_dlssnr.dll`.
//!
//! Only a file whose SHA-256 is one of the pinned versions is accepted. It is
//! looked for in a fixed order: a path the person gives, then the folders
//! games are installed in (Steam libraries, Heroic installs, local games),
//! then installed Ferestre Xbox packages (the files their install already
//! decrypted with the person's licence), then a URL from the person's own
//! settings. This repository never holds such a URL.
//!
//! The DLL is only read for its model; a downloaded or unzipped copy is
//! deleted when the [`Accepted`] holding it is dropped.

use super::{DllVersion, Expect};
use crate::paths::Paths;
use crate::recipe::Recipe;
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The file looked for, matched case-insensitively.
pub const DLL_NAME: &str = "nvngx_dlssnr.dll";
/// Neither a DLL nor its download is ever larger than this.
pub const MAX_DLL: u64 = 512 * 1024 * 1024;
/// A file of the right name and a size outside this is not hashed.
const DLL_SIZES: RangeInclusive<u64> = 50 * 1024 * 1024..=MAX_DLL;
/// Directories a scan never enters: prefixes, caches and partial downloads,
/// and Ferestre's own backups.
const SKIPPED_DIRS: &[&str] = &[
    "compatdata",
    "shadercache",
    "downloading",
    "temp",
    ".ferestre-dlssnr-backup",
];
const XBOX_NOTE: &str = "(this copy may be encrypted on disk or still downloading)";

/// Files deleted when dropped.
#[derive(Debug, Default)]
struct Removed(Vec<PathBuf>);

impl Drop for Removed {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// An accepted DLL. `path` is where it can be read while this lives; a copy
/// Ferestre made (unzipped or downloaded) is deleted on drop.
#[derive(Debug)]
pub struct Accepted {
    pub path: PathBuf,
    pub version: DllVersion,
    /// The file, zip or URL it came from.
    pub found_at: PathBuf,
    _work: Option<Removed>,
}

/// A file named like the DLL that was not accepted, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub path: PathBuf,
    pub reason: String,
}

/// How far one scan may go.
#[derive(Debug, Clone)]
pub struct Budget {
    pub deadline: Instant,
    /// Directory entries still allowed.
    pub entries: usize,
    /// How many directories deep below a root.
    pub depth: usize,
}

impl Budget {
    /// 30 seconds, 400,000 entries, 12 levels.
    pub fn standard() -> Self {
        Budget {
            deadline: Instant::now() + Duration::from_secs(30),
            entries: 400_000,
            depth: 12,
        }
    }

    fn spent(&self) -> bool {
        self.entries == 0 || Instant::now() >= self.deadline
    }
}

fn is_zip(path: &Path) -> Result<bool> {
    let mut magic = [0u8; 4];
    let mut file =
        std::fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let n = file.read(&mut magic)?;
    Ok(n == 4 && magic == *b"PK\x03\x04")
}

/// Unzip the one `nvngx_dlssnr.dll` of `zip` to `out`.
fn extract_one(zip: &Path, out: &Path) -> Result<()> {
    let file =
        std::fs::File::open(zip).with_context(|| format!("cannot read {}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable zip", zip.display()))?;
    let mut found = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let base = entry.name().rsplit(['/', '\\']).next().unwrap_or_default();
        if !entry.is_dir() && base.eq_ignore_ascii_case(DLL_NAME) {
            found.push(i);
        }
    }
    let index = match found.as_slice() {
        [index] => *index,
        [] => bail!("{} holds no {DLL_NAME}", zip.display()),
        more => bail!(
            "{} holds {} copies of {DLL_NAME}; unzip the one to use and give that",
            zip.display(),
            more.len()
        ),
    };
    let entry = archive.by_index(index)?;
    if entry.size() > MAX_DLL {
        bail!("{DLL_NAME} in {} is too large", zip.display());
    }
    let mut dest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .with_context(|| format!("cannot write {}", out.display()))?;
    let copied = std::io::copy(&mut entry.take(MAX_DLL + 1), &mut dest)
        .with_context(|| format!("cannot unzip {}", zip.display()))?;
    if copied > MAX_DLL {
        bail!("{DLL_NAME} in {} is too large", zip.display());
    }
    Ok(())
}

/// The accepted version whose hash `file` has; the error names `shown`.
fn version_of(file: &Path, shown: &Path, expect: &Expect) -> Result<DllVersion> {
    let sha256 = crate::updater::sha256_file(file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    if let Some(version) = expect.dlls.iter().find(|v| v.sha256 == sha256) {
        return Ok(version.clone());
    }
    let versions: Vec<&str> = expect.dlls.iter().map(|v| v.version.as_str()).collect();
    bail!(
        "{} is not {DLL_NAME} {} (SHA-256 {sha256})",
        shown.display(),
        versions.join(" or ")
    )
}

/// Accept `candidate`: the DLL itself, or a zip holding exactly one
/// `nvngx_dlssnr.dll` (in any folder), which is unzipped into `work`.
pub fn accept(candidate: &Path, work: &Path, expect: &Expect) -> Result<Accepted> {
    if !is_zip(candidate)? {
        let version = version_of(candidate, candidate, expect)?;
        return Ok(Accepted {
            path: candidate.to_path_buf(),
            version,
            found_at: candidate.to_path_buf(),
            _work: None,
        });
    }
    std::fs::create_dir_all(work).with_context(|| format!("{}", work.display()))?;
    let out = crate::cache::temporary_beside(&work.join(DLL_NAME), "dll");
    let guard = Removed(vec![out.clone()]);
    extract_one(candidate, &out)?;
    let version = version_of(&out, candidate, expect)?;
    Ok(Accepted {
        path: out,
        version,
        found_at: candidate.to_path_buf(),
        _work: Some(guard),
    })
}

/// Look for an accepted DLL under `roots`, in order, within `budget`. Stops
/// at the first accepted one; every other file of that name is a rejection.
pub fn scan(
    roots: &[PathBuf],
    work: &Path,
    budget: &mut Budget,
    expect: &Expect,
) -> (Option<Accepted>, Vec<Rejected>) {
    scan_sized(roots, work, budget, expect, &DLL_SIZES)
}

fn scan_sized(
    roots: &[PathBuf],
    work: &Path,
    budget: &mut Budget,
    expect: &Expect,
    sizes: &RangeInclusive<u64>,
) -> (Option<Accepted>, Vec<Rejected>) {
    let mut rejected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let real = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if !seen.insert(real) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            let mut below = Vec::new();
            for entry in entries {
                if budget.spent() {
                    return (None, rejected);
                }
                budget.entries -= 1;
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let path = entry.path();
                if kind.is_dir() {
                    let lower = name.to_ascii_lowercase();
                    if depth < budget.depth && !SKIPPED_DIRS.contains(&lower.as_str()) {
                        below.push((path, depth + 1));
                    }
                    continue;
                }
                if !name.eq_ignore_ascii_case(DLL_NAME) {
                    continue;
                }
                // A link to a file is followed; a link to a directory is not
                // (it is neither entered nor a file).
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                if !sizes.contains(&meta.len()) {
                    rejected.push(Rejected {
                        path,
                        reason: format!("{} bytes is not the size of {DLL_NAME}", meta.len()),
                    });
                    continue;
                }
                match accept(&path, work, expect) {
                    Ok(accepted) => return (Some(accepted), rejected),
                    Err(e) => rejected.push(Rejected {
                        path,
                        reason: format!("{e:#}"),
                    }),
                }
            }
            // Visited in name order.
            stack.extend(below.into_iter().rev());
        }
    }
    (None, rejected)
}

/// Where to look, by discovery step. Only existing directories, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locations {
    pub steam: Vec<PathBuf>,
    pub heroic: Vec<PathBuf>,
    pub local: Vec<PathBuf>,
    pub xbox: Vec<PathBuf>,
}

fn existing(dirs: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    dirs.into_iter()
        .filter(|dir| dir.is_dir())
        .filter(|dir| seen.insert(dir.canonicalize().unwrap_or_else(|_| dir.clone())))
        .collect()
}

/// Every `install_path` or `path` string in a Heroic `installed.json`,
/// whatever its shape (Legendary's map, GOG's list, Nile's list).
fn heroic_paths(value: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match value {
                    serde_json::Value::String(path)
                        if (key == "install_path" || key == "path") && !path.is_empty() =>
                    {
                        out.push(PathBuf::from(path));
                    }
                    other => heroic_paths(other, out),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                heroic_paths(item, out);
            }
        }
        _ => {}
    }
}

/// The folders each discovery step scans.
pub fn locations(paths: &Paths, recipes: &[Recipe]) -> Locations {
    let steam = paths
        .steam_roots()
        .into_iter()
        .flat_map(|root| crate::steam::libraries(&root))
        .map(|library| library.join("steamapps").join("common"));

    let xdg_config = paths
        .config_dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| paths.home().join(".config"));
    let mut heroic = Vec::new();
    for base in [
        xdg_config.join("heroic"),
        paths
            .home()
            .join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
    ] {
        for file in [
            "legendaryConfig/legendary/installed.json",
            "gog_store/installed.json",
            "nile_config/nile/installed.json",
        ] {
            if let Some(value) = std::fs::read_to_string(base.join(file))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
            {
                heroic_paths(&value, &mut heroic);
            }
        }
    }
    heroic.push(paths.home().join("Games/Heroic"));

    let mut local = Vec::new();
    let mut xbox: Vec<PathBuf> = crate::install::all(paths.state_dir())
        .into_iter()
        .map(|record| record.dir)
        .collect();
    for recipe in recipes {
        if let Ok(dir) = paths.install_dir(recipe) {
            if crate::install::looks_installed(&dir) {
                xbox.push(dir);
            } else {
                local.push(dir);
            }
        }
    }
    xbox.push(paths.games_dir().to_path_buf());

    Locations {
        steam: existing(steam),
        heroic: existing(heroic),
        local: existing(local),
        xbox: existing(xbox),
    }
}

/// The discovery step a DLL was found by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Explicit,
    Scan,
    Xbox,
    Url,
}

/// Find an accepted DLL: `explicit` (a rejection is the error), else the
/// game folders, else installed Xbox packages, else `url`. The error says
/// what was tried, every rejection, and how to give a DLL.
pub fn find(
    locations: &Locations,
    explicit: Option<&Path>,
    url: Option<&str>,
    work: &Path,
    expect: &Expect,
    progress: &mut dyn FnMut(&str),
) -> Result<(Accepted, Step)> {
    find_with(
        locations,
        explicit,
        url,
        work,
        expect,
        progress,
        &|url, dest| crate::http::download_large_checked(url, dest, MAX_DLL, || false, |_| Ok(())),
        &DLL_SIZES,
    )
}

#[allow(clippy::too_many_arguments)]
fn find_with(
    locations: &Locations,
    explicit: Option<&Path>,
    url: Option<&str>,
    work: &Path,
    expect: &Expect,
    progress: &mut dyn FnMut(&str),
    download: &dyn Fn(&str, &Path) -> Result<()>,
    sizes: &RangeInclusive<u64>,
) -> Result<(Accepted, Step)> {
    if let Some(path) = explicit {
        progress(&format!("Checking {}", path.display()));
        return accept(path, work, expect).map(|accepted| (accepted, Step::Explicit));
    }

    let mut tried = Vec::new();
    let mut rejected = Vec::new();

    let games: Vec<PathBuf> = locations
        .steam
        .iter()
        .chain(&locations.heroic)
        .chain(&locations.local)
        .cloned()
        .collect();
    progress(&format!(
        "Looking for {DLL_NAME} in {} game folders",
        games.len()
    ));
    let mut budget = Budget::standard();
    let (found, rejections) = scan_sized(&games, work, &mut budget, expect, sizes);
    if let Some(accepted) = found {
        return Ok((accepted, Step::Scan));
    }
    tried.push(format!(
        "{} game folders (Steam libraries, Heroic installs, local games){}",
        games.len(),
        if budget.spent() {
            ", stopped at the scan's time or size limit"
        } else {
            ""
        }
    ));
    rejected.extend(rejections);

    progress(&format!(
        "Looking for {DLL_NAME} in {} installed Xbox package folders",
        locations.xbox.len()
    ));
    let mut budget = Budget::standard();
    let (found, rejections) = scan_sized(&locations.xbox, work, &mut budget, expect, sizes);
    if let Some(accepted) = found {
        return Ok((accepted, Step::Xbox));
    }
    tried.push(format!(
        "{} installed Xbox package folders{}",
        locations.xbox.len(),
        if budget.spent() {
            ", stopped at the scan's time or size limit"
        } else {
            ""
        }
    ));
    rejected.extend(rejections.into_iter().map(|r| Rejected {
        reason: format!("{} {XBOX_NOTE}", r.reason),
        ..r
    }));

    match url {
        Some(url) => {
            progress(&format!("Downloading {DLL_NAME} from the configured URL"));
            match fetch(url, work, expect, download) {
                Ok(accepted) => return Ok((accepted, Step::Url)),
                Err(e) => tried.push(format!("the configured URL: {e:#}")),
            }
        }
        None => tried.push("no URL is configured".to_string()),
    }

    let mut message = format!(
        "{DLL_NAME} {} was not found.\nTried:",
        expect
            .dlls
            .iter()
            .map(|v| v.version.as_str())
            .collect::<Vec<_>>()
            .join(" or ")
    );
    for step in &tried {
        message.push_str(&format!("\n  - {step}"));
    }
    if !rejected.is_empty() {
        message.push_str("\nNot accepted:");
        for r in &rejected {
            message.push_str(&format!("\n  - {}: {}", r.path.display(), r.reason));
        }
    }
    message.push_str(&format!(
        "\nGive the DLL (or a .zip holding it) with --dll <path>, or set an https:// URL for it \
         as dll-url under [dlssnr] in Ferestre's {} or in {}.",
        crate::settings::FILE,
        crate::settings::DLL_URL_ENV
    ));
    bail!(message)
}

/// Download `url` into `work` and accept it; the download is deleted with
/// the result, or at once when it is not accepted.
fn fetch(
    url: &str,
    work: &Path,
    expect: &Expect,
    download: &dyn Fn(&str, &Path) -> Result<()>,
) -> Result<Accepted> {
    std::fs::create_dir_all(work).with_context(|| format!("{}", work.display()))?;
    let dest = crate::cache::temporary_beside(&work.join("dll-download"), "bin");
    let mut guard = Removed(vec![dest.clone()]);
    download(url, &dest)?;
    let mut accepted = accept(&dest, work, expect)?;
    if let Some(mut own) = accepted._work.take() {
        guard.0.append(&mut std::mem::take(&mut own.0));
    }
    accepted.found_at = PathBuf::from(url);
    accepted._work = Some(guard);
    Ok(accepted)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::neural::ModelFile;
    use crate::paths::Env;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    pub(crate) const FAKE_NEW: &[u8] = b"fake dll 310.9.16";
    pub(crate) const FAKE_OLD: &[u8] = b"fake dll 310.8.0";
    const ANY_SIZE: RangeInclusive<u64> = 1..=MAX_DLL;

    pub(crate) fn hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Accepts [`FAKE_NEW`] and [`FAKE_OLD`]; models `aardwold`/`small`.
    pub(crate) fn expect_with(aardwold: &[u8], small: &[u8]) -> Expect {
        Expect {
            dlls: vec![
                DllVersion {
                    version: "310.9.16".into(),
                    sha256: hex(FAKE_NEW),
                    small_model: true,
                },
                DllVersion {
                    version: "310.8.0".into(),
                    sha256: hex(FAKE_OLD),
                    small_model: false,
                },
            ],
            aardwold: ModelFile {
                file: "CC_SILVER_AARDWOLD.bin".into(),
                sha256: hex(aardwold),
                size: aardwold.len() as u64,
            },
            small: ModelFile {
                file: "CC_SMALL_FP8.bin".into(),
                sha256: hex(small),
                size: small.len() as u64,
            },
        }
    }

    fn expect() -> Expect {
        expect_with(b"aardwold", b"small")
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn files_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_dll_with_an_accepted_hash_is_accepted_and_others_say_their_hash() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let good = dir.path().join("nvngx_dlssnr.dll");
        write(&good, FAKE_OLD);
        let accepted = accept(&good, &work, &expect()).unwrap();
        assert_eq!(accepted.version.version, "310.8.0");
        assert_eq!(accepted.path, good);
        drop(accepted);
        assert!(good.is_file(), "a DLL found in place is never deleted");

        let bad = dir.path().join("other.dll");
        write(&bad, b"something else");
        let err = accept(&bad, &work, &expect()).unwrap_err().to_string();
        assert_eq!(
            err,
            format!(
                "{} is not nvngx_dlssnr.dll 310.9.16 or 310.8.0 (SHA-256 {})",
                bad.display(),
                hex(b"something else")
            )
        );
    }

    #[test]
    fn a_zip_must_hold_exactly_one_nvngx_dlssnr_dll() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");

        let one = dir.path().join("one.zip");
        zip_of(
            &one,
            &[("readme.txt", b"hi"), ("sub/NVNGX_DLSSNR.DLL", FAKE_NEW)],
        );
        let accepted = accept(&one, &work, &expect()).unwrap();
        assert_eq!(accepted.version.version, "310.9.16");
        assert_eq!(accepted.found_at, one);
        assert!(accepted.path.starts_with(&work));
        let unzipped = accepted.path.clone();
        assert!(unzipped.is_file());
        drop(accepted);
        assert!(!unzipped.exists(), "the unzipped copy is not kept");

        let two = dir.path().join("two.zip");
        zip_of(
            &two,
            &[
                ("a/nvngx_dlssnr.dll", FAKE_NEW),
                ("b/nvngx_dlssnr.dll", FAKE_OLD),
            ],
        );
        let err = accept(&two, &work, &expect()).unwrap_err().to_string();
        assert!(err.contains("2 copies"), "{err}");

        let none = dir.path().join("none.zip");
        zip_of(&none, &[("nvngx_dlss.dll", FAKE_NEW)]);
        let err = accept(&none, &work, &expect()).unwrap_err().to_string();
        assert!(err.contains("holds no"), "{err}");

        let wrong = dir.path().join("wrong.zip");
        zip_of(&wrong, &[("nvngx_dlssnr.dll", b"nope")]);
        let err = accept(&wrong, &work, &expect()).unwrap_err().to_string();
        assert!(err.starts_with(&wrong.display().to_string()), "{err}");
        assert!(files_in(&work).is_empty(), "{:?}", files_in(&work));
    }

    #[test]
    fn the_scan_finds_a_dll_deep_in_a_library_and_skips_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let common = dir.path().join("lib/steamapps/common");
        let deep = common
            .join("Game/Engine/Plugins/Runtime/Nvidia/Binaries/ThirdParty/Win64/nvngx_dlssnr.dll");
        write(&deep, FAKE_NEW);
        // Wrong copies where a scan must never look: none of them is hashed,
        // so none is a rejection.
        let skipped = dir.path().join("lib2/steamapps");
        for sub in [
            "compatdata/123/pfx/drive_c/nvngx_dlssnr.dll",
            "shadercache/1/nvngx_dlssnr.dll",
            "common/Game/.ferestre-dlssnr-backup/nvngx_dlssnr.dll",
            "downloading/1/nvngx_dlssnr.dll",
        ] {
            write(&skipped.join(sub), b"wrong");
        }
        let roots = vec![skipped.clone(), common.clone()];
        let (found, rejected) =
            scan_sized(&roots, &work, &mut Budget::standard(), &expect(), &ANY_SIZE);
        assert_eq!(rejected, vec![]);
        assert_eq!(found.unwrap().found_at, deep);

        // The real size limits are applied before any hashing.
        let (found, rejected) = scan(&roots, &work, &mut Budget::standard(), &expect());
        assert!(found.is_none());
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("not the size"), "{rejected:?}");
    }

    fn paths_for(home: &Path) -> Paths {
        let s = |p: PathBuf| p.to_str().unwrap().to_string();
        Paths::resolve(&Env::from_map([
            ("HOME".to_string(), s(home.to_path_buf())),
            ("XDG_CONFIG_HOME".to_string(), s(home.join("config"))),
            ("XDG_STATE_HOME".to_string(), s(home.join("state"))),
            ("XDG_CACHE_HOME".to_string(), s(home.join("cache"))),
            ("XDG_DATA_HOME".to_string(), s(home.join("data"))),
            ("XODUS_GAMES_DIR".to_string(), s(home.join("games"))),
        ]))
        .unwrap()
    }

    #[test]
    fn every_steam_root_and_library_is_a_location() {
        let home = tempfile::tempdir().unwrap();
        let native = home.path().join(".local/share/Steam");
        let flatpak = home
            .path()
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        let third = home.path().join("drive/SteamLibrary");
        for root in [&native, &flatpak, &third] {
            std::fs::create_dir_all(root.join("steamapps/common")).unwrap();
        }
        write(
            &flatpak.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                flatpak.display(),
                third.display()
            )
            .as_bytes(),
        );
        let locations = locations(&paths_for(home.path()), &[]);
        assert_eq!(
            locations.steam,
            vec![
                native.join("steamapps/common"),
                flatpak.join("steamapps/common"),
                third.join("steamapps/common"),
            ]
        );
    }

    #[test]
    fn heroic_installs_are_locations() {
        let home = tempfile::tempdir().unwrap();
        let legendary = home.path().join("Games/Heroic/Fortnite-ish");
        let gog = home.path().join("gog/Witcher");
        std::fs::create_dir_all(&legendary).unwrap();
        std::fs::create_dir_all(&gog).unwrap();
        write(
            &home
                .path()
                .join("config/heroic/legendaryConfig/legendary/installed.json"),
            serde_json::json!({ "App": { "install_path": legendary } })
                .to_string()
                .as_bytes(),
        );
        write(
            &home
                .path()
                .join(".var/app/com.heroicgameslauncher.hgl/config/heroic/gog_store/installed.json"),
            serde_json::json!({ "installed": [{ "install_path": gog }, { "install_path": "/nowhere" }] })
                .to_string()
                .as_bytes(),
        );
        let locations = locations(&paths_for(home.path()), &[]);
        assert_eq!(
            locations.heroic,
            vec![legendary, gog, home.path().join("Games/Heroic")]
        );
        assert!(locations.steam.is_empty() && locations.local.is_empty());
        assert!(locations.xbox.is_empty(), "the games dir does not exist");
    }

    #[test]
    fn a_scan_stops_at_its_budget() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..100 {
            write(&dir.path().join(format!("d{i:03}/file")), b"x");
        }
        write(&dir.path().join("zzz/nvngx_dlssnr.dll"), FAKE_NEW);
        let mut budget = Budget {
            entries: 10,
            ..Budget::standard()
        };
        let (found, rejected) = scan_sized(
            &[dir.path().to_path_buf()],
            &dir.path().join("work"),
            &mut budget,
            &expect(),
            &ANY_SIZE,
        );
        assert!(found.is_none());
        assert!(rejected.is_empty());
        assert_eq!(budget.entries, 0);

        let mut budget = Budget {
            deadline: Instant::now(),
            ..Budget::standard()
        };
        let (found, _) = scan_sized(
            &[dir.path().to_path_buf()],
            &dir.path().join("work"),
            &mut budget,
            &expect(),
            &ANY_SIZE,
        );
        assert!(found.is_none());
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir_all(a.join("b")).unwrap();
        std::os::unix::fs::symlink(&a, a.join("b/loop")).unwrap();
        std::os::unix::fs::symlink(dir.path(), a.join("up")).unwrap();
        write(&a.join("b/nvngx_dlssnr.dll"), FAKE_NEW);
        let mut budget = Budget {
            depth: 1000,
            entries: usize::MAX,
            ..Budget::standard()
        };
        // The root twice, once through a link: still one visit.
        let roots = vec![a.clone(), a.join("b/loop")];
        let (found, rejected) = scan_sized(
            &roots,
            &dir.path().join("work"),
            &mut budget,
            &expect(),
            &ANY_SIZE,
        );
        assert_eq!(found.unwrap().found_at, a.join("b/nvngx_dlssnr.dll"));
        assert!(rejected.is_empty());
    }

    /// Serves `body` to every request, until the test ends.
    fn serve(body: &'static [u8]) -> String {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in server.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/file")
    }

    fn plain_download(url: &str, dest: &Path) -> Result<()> {
        let mut response = ureq::get(url).call()?;
        std::fs::write(dest, response.body_mut().read_to_vec()?)?;
        Ok(())
    }

    fn find_here(
        locations: &Locations,
        explicit: Option<&Path>,
        url: Option<&str>,
        work: &Path,
    ) -> Result<(Accepted, Step)> {
        find_with(
            locations,
            explicit,
            url,
            work,
            &expect(),
            &mut |_| {},
            &plain_download,
            &ANY_SIZE,
        )
    }

    #[test]
    fn discovery_tries_explicit_then_games_then_xbox_then_url() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let game = dir.path().join("game");
        let xbox = dir.path().join("xbox");
        write(&game.join("nvngx_dlssnr.dll"), FAKE_NEW);
        write(&xbox.join("Title/nvngx_dlssnr.dll"), FAKE_OLD);
        let bad = dir.path().join("bad.dll");
        write(&bad, b"bad");

        let both = Locations {
            steam: vec![game.clone()],
            xbox: vec![xbox.clone()],
            ..Locations::default()
        };
        let err = find_here(&both, Some(&bad), None, &work)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not nvngx_dlssnr.dll"), "{err}");

        let (accepted, step) = find_here(&both, None, None, &work).unwrap();
        assert_eq!(step, Step::Scan);
        assert_eq!(accepted.version.version, "310.9.16");

        let only_xbox = Locations {
            xbox: vec![xbox.clone()],
            ..Locations::default()
        };
        let (accepted, step) = find_here(&only_xbox, None, None, &work).unwrap();
        assert_eq!(step, Step::Xbox);
        assert_eq!(accepted.version.version, "310.8.0");

        let url = serve(FAKE_NEW);
        let (accepted, step) = find_here(&Locations::default(), None, Some(&url), &work).unwrap();
        assert_eq!(step, Step::Url);
        assert_eq!(accepted.found_at, PathBuf::from(&url));
        assert!(accepted.path.is_file());
        drop(accepted);
        assert!(files_in(&work).is_empty(), "{:?}", files_in(&work));

        let err = find_here(&Locations::default(), None, None, &work)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no URL is configured"), "{err}");
        assert!(err.contains("--dll"), "{err}");
        assert!(err.contains(crate::settings::DLL_URL_ENV), "{err}");
    }

    #[test]
    fn a_zip_from_the_url_is_unzipped_and_nothing_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("dll.zip");
        zip_of(&zip, &[("x/nvngx_dlssnr.dll", FAKE_NEW)]);
        let body: &'static [u8] = Box::leak(std::fs::read(&zip).unwrap().into_boxed_slice());
        let url = serve(body);
        let work = dir.path().join("work");
        let (accepted, step) = find_here(&Locations::default(), None, Some(&url), &work).unwrap();
        assert_eq!(step, Step::Url);
        assert_eq!(
            files_in(&work).len(),
            2,
            "the download and the unzipped DLL"
        );
        drop(accepted);
        assert!(files_in(&work).is_empty(), "{:?}", files_in(&work));

        // A download that is not accepted is not kept either.
        let url = serve(b"not it");
        let err = find_here(&Locations::default(), None, Some(&url), &work)
            .unwrap_err()
            .to_string();
        assert!(err.contains("the configured URL"), "{err}");
        assert!(files_in(&work).is_empty(), "{:?}", files_in(&work));
    }

    #[test]
    fn an_encrypted_xbox_copy_is_reported_not_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let xbox = dir.path().join("xbox");
        write(&xbox.join("Title/nvngx_dlssnr.dll"), b"encrypted bytes");
        let only_xbox = Locations {
            xbox: vec![xbox.clone()],
            ..Locations::default()
        };
        let err = find_here(&only_xbox, None, None, &dir.path().join("work"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("encrypted on disk or still downloading"),
            "{err}"
        );
        assert!(err.contains(&xbox.join("Title/nvngx_dlssnr.dll").display().to_string()));
    }
}
