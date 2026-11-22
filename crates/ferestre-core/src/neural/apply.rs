//! Turning neural rendering on and off for one game, and keeping it right
//! before every launch.
//!
//! The mod goes into the executable's own folder. Before upstream's
//! `install.sh` writes there, whatever it would overwrite that is not
//! Ferestre's own (another mod's `dxgi.dll`, a `ReShade.ini`, a
//! `reshade-shaders/` folder) is copied into [`BACKUP_DIR`] and listed in its
//! `manifest.json`, so turning it off puts back byte-identical files. A
//! restore validates that manifest whole before it deletes anything.
//!
//! The anti-cheat gate runs on enable and before every launch: a kernel-level
//! anti-cheat Proton cannot run refuses outright, any other needs an
//! acknowledgement remembered per game and anti-cheat.

use super::detect::{self, Detected};
use super::package::{self, RouteFile};
use super::state::{self, Entry};
use super::{hardware, models, Arch, Context, Expect, ModelFile, Network, Route};
use crate::anticheat::{self, Detection};
use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// Where the person's files are kept while the mod is in, inside the
/// executable's folder.
pub const BACKUP_DIR: &str = ".ferestre-dlssnr-backup";
/// The list of what upstream's `install.sh` wrote, in the executable's folder.
pub const INSTALL_MANIFEST: &str = "dlssnr-amd-install.txt";
const BACKUP_MANIFEST: &str = "manifest.json";
const BACKUP_SCHEMA: u32 = 1;
const INI: &str = "dlssnr-amd.ini";
/// The person's `dlssnr-amd.ini`, kept across a reinstall (which writes a
/// fresh one).
const KEPT_INI: &str = "dlssnr-amd.ini.user";
const INT4_MODEL: &str = "dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin";
const INT4_LAYER_DIR: &str = "dlssnr-amd/int4/layer";
const INT4_LAYER: &str = "VK_LAYER_DLSSNR_int4";
const LOG_TAIL: usize = 20;

/// The folder the executable is in: where the mod goes. A relative
/// executable (Windows or POSIX separators) is under `game_dir`; an absolute
/// one is taken as it is.
pub fn exe_dir(game_dir: &Path, executable: &str) -> PathBuf {
    let exe = PathBuf::from(executable.replace('\\', "/"));
    let full = if exe.is_absolute() {
        exe
    } else {
        game_dir.join(exe)
    };
    full.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| game_dir.to_path_buf())
}

/// What the anti-cheat found in a game allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allowed,
    /// Anti-cheats the person has not acknowledged for this game.
    NeedsAcknowledgement(Vec<&'static str>),
    /// Kernel-level anti-cheats Proton cannot run: never, no override.
    Refused(Vec<&'static str>),
}

pub fn gate(found: &[Detection], acknowledged: &BTreeSet<String>) -> Gate {
    let mut refused: Vec<&'static str> = Vec::new();
    let mut unacknowledged: Vec<&'static str> = Vec::new();
    for detection in found {
        let list = if detection.kernel && !detection.proton_runtime {
            &mut refused
        } else if !acknowledged.contains(detection.name) {
            &mut unacknowledged
        } else {
            continue;
        };
        if !list.contains(&detection.name) {
            list.push(detection.name);
        }
    }
    if !refused.is_empty() {
        Gate::Refused(refused)
    } else if !unacknowledged.is_empty() {
        Gate::NeedsAcknowledgement(unacknowledged)
    } else {
        Gate::Allowed
    }
}

/// Why a launch does not get neural rendering, or `None` when it may.
fn gate_reason(gate: &Gate) -> Option<String> {
    match gate {
        Gate::Allowed => None,
        Gate::Refused(names) => Some(format!(
            "{} is a kernel-level anti-cheat that Proton cannot run; neural rendering is never used with it",
            names.join(", ")
        )),
        Gate::NeedsAcknowledgement(names) => Some(format!(
            "{} was found and is not acknowledged for this game; to use neural rendering anyway, \
             turn it on again with --accept-anticheat-risk",
            names.join(", ")
        )),
    }
}

fn game_name(game_dir: &Path, key: &str) -> String {
    game_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| key.to_string())
}

fn log_file(ctx: &Context, key: &str) -> PathBuf {
    ctx.root.join("logs").join(format!("{}.log", safe_key(key)))
}

/// A key as a file name: product ids are already plain, but a state file
/// someone edited must not name a path.
fn safe_key(key: &str) -> String {
    key.to_ascii_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// An exclusive lock per game, held until the file is dropped, so a launch
/// and an enable or disable never install into one folder at once.
fn lock(ctx: &Context, key: &str) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let dir = ctx.root.join("locks");
    std::fs::create_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
    let path = dir.join(format!("{}.lock", safe_key(key)));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("{}", path.display()))?;
    loop {
        // SAFETY: the descriptor belongs to `file`, which outlives the lock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error).with_context(|| format!("cannot lock {}", path.display()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    path: String,
    dir: bool,
}

/// `<exe dir>/.ferestre-dlssnr-backup/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupManifest {
    schema: u32,
    package: String,
    route: Route,
    /// What was there before and is kept in [`BACKUP_DIR`].
    saved: Vec<Saved>,
    /// What was not there before.
    absent: Vec<String>,
    /// The sha256 of each file the last install copied from the package, so
    /// a reinstall from another package still knows Ferestre's own older
    /// copy from the game's. Never used to decide what to delete.
    #[serde(default)]
    installed: BTreeMap<String, String>,
}

fn read_backup(exe_dir: &Path) -> Result<Option<BackupManifest>> {
    let path = exe_dir.join(BACKUP_DIR).join(BACKUP_MANIFEST);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => bail!("{}: {e}", path.display()),
    };
    let manifest: BackupManifest =
        serde_json::from_str(&text).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    if manifest.schema != BACKUP_SCHEMA {
        bail!(
            "{}: schema {} is not one this Ferestre knows",
            path.display(),
            manifest.schema
        );
    }
    Ok(Some(manifest))
}

fn write_backup(exe_dir: &Path, manifest: &BackupManifest) -> Result<()> {
    crate::cache::write(
        &exe_dir.join(BACKUP_DIR).join(BACKUP_MANIFEST),
        &serde_json::to_vec_pretty(manifest)?,
    )
}

/// The top-level names upstream's install manifest lists.
fn installed_names(exe_dir: &Path) -> BTreeSet<String> {
    let Ok(text) = std::fs::read_to_string(exe_dir.join(INSTALL_MANIFEST)) else {
        return BTreeSet::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim().replace('\\', "/");
            match Path::new(&line).components().next() {
                Some(Component::Normal(first)) => Some(first.to_string_lossy().into_owned()),
                _ => None,
            }
        })
        .collect()
}

fn names(files: &[RouteFile]) -> BTreeSet<String> {
    files.iter().map(|f| f.name.clone()).collect()
}

/// Whether two regular files have the same bytes.
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if !ma.is_file() || !mb.is_file() || ma.len() != mb.len() {
        return false;
    }
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut fa, mut fb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let Ok(n) = fa.read(&mut ba) else {
            return false;
        };
        if n == 0 {
            // Same length, so the other is at its end too.
            return true;
        }
        if fb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
}

/// Remove a file, a folder or a link (never followed). Nothing there is fine.
fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    }
}

/// Copy a file or a folder; links inside a folder are copied as links.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Copy whatever the route would overwrite that is not Ferestre's own into
/// [`BACKUP_DIR`], merging with an earlier backup: a name already saved is
/// never saved again (so a reinstall never backs up the mod's own files over
/// the person's), and a name that was absent before stays absent unless a
/// file the install copies is there now, differs from the package's and is
/// no longer in the install manifest (the game put its own there).
pub fn backup(exe_dir: &Path, files: &[RouteFile], package: &str, route: Route) -> Result<()> {
    let dir = exe_dir.join(BACKUP_DIR);
    let previous = read_backup(exe_dir)?;
    if let Some(previous) = &previous {
        if previous.route != route {
            bail!(
                "neural rendering is installed in {} with the {} route; turn it off before choosing another",
                exe_dir.display(),
                previous.route
            );
        }
    }
    let (mut saved, mut absent, installed) = previous
        .map(|p| (p.saved, p.absent, p.installed))
        .unwrap_or_default();
    let listed = installed_names(exe_dir);
    let fresh = !dir.exists();
    std::fs::create_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
    let result = (|| -> Result<()> {
        for file in files {
            if file.name == BACKUP_DIR || saved.iter().any(|s| s.path == file.name) {
                continue;
            }
            let here = exe_dir.join(&file.name);
            let was_absent = absent.contains(&file.name);
            let Ok(meta) = std::fs::symlink_metadata(&here) else {
                if !was_absent {
                    absent.push(file.name.clone());
                }
                continue;
            };
            let copies_this_file = !file.dir && file.source.is_some();
            // The package's copy, or the one an earlier install wrote (the
            // package may have changed since).
            let ours = copies_this_file
                && (file
                    .source
                    .as_deref()
                    .is_some_and(|source| same_bytes(&here, source))
                    || installed.get(&file.name).is_some_and(|hash| {
                        meta.is_file()
                            && crate::updater::sha256_file(&here).is_ok_and(|h| &h == hash)
                    }));
            let foreign = if was_absent {
                copies_this_file && !listed.contains(&file.name) && !ours
            } else if file.only_if_absent {
                // `install.sh` leaves the game's own copy alone.
                continue;
            } else {
                !listed.contains(&file.name) && !ours
            };
            if !foreign {
                if !was_absent {
                    absent.push(file.name.clone());
                }
                continue;
            }
            if meta.file_type().is_symlink() {
                bail!(
                    "{} is a symbolic link; neural rendering does not install over it",
                    here.display()
                );
            }
            let copy = dir.join(&file.name);
            remove_any(&copy).with_context(|| format!("{}", copy.display()))?;
            copy_tree(&here, &copy)
                .with_context(|| format!("cannot back up {}", here.display()))?;
            saved.push(Saved {
                path: file.name.clone(),
                dir: meta.is_dir(),
            });
            absent.retain(|a| a != &file.name);
        }
        let manifest = BackupManifest {
            schema: BACKUP_SCHEMA,
            package: package.to_string(),
            route,
            saved: saved.clone(),
            absent: absent.clone(),
            installed: installed.clone(),
        };
        write_backup(exe_dir, &manifest)
    })();
    if result.is_err() && fresh {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Remember the sha256 of each file the install just copied from the
/// package, in the backup manifest.
fn record_installed(exe_dir: &Path, files: &[RouteFile]) -> Result<()> {
    let Some(mut manifest) = read_backup(exe_dir)? else {
        return Ok(());
    };
    manifest.installed.clear();
    for file in files {
        let Some(source) = file.source.as_deref().filter(|_| !file.dir) else {
            continue;
        };
        if same_bytes(&exe_dir.join(&file.name), source) {
            manifest
                .installed
                .insert(file.name.clone(), crate::updater::sha256_file(source)?);
        }
    }
    write_backup(exe_dir, &manifest)
}

/// Why a name in the backup manifest is not one restore may touch.
fn check_name(name: &str, allowed: &BTreeSet<String>) -> Result<(), String> {
    let mut parts = Path::new(name).components();
    let single = matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    );
    if !single || name.contains('/') || name.contains('\\') || name == BACKUP_DIR {
        return Err(format!("{name:?} is not a plain file name"));
    }
    if !allowed.contains(name) {
        return Err(format!("{name:?} is not a file neural rendering writes"));
    }
    Ok(())
}

/// The backup manifest, checked whole: `None` when there is no backup.
fn validated(exe_dir: &Path, allowed: &BTreeSet<String>) -> Result<Option<BackupManifest>> {
    let dir = exe_dir.join(BACKUP_DIR);
    let manifest_path = dir.join(BACKUP_MANIFEST);
    let refuse = |why: String| {
        anyhow!(
            "{}: {why}; nothing was changed, restore your files by hand from {}",
            manifest_path.display(),
            dir.display()
        )
    };
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(refuse(e.to_string())),
        Ok(meta) if !meta.is_dir() => return Err(refuse(format!("{BACKUP_DIR} is not a folder"))),
        Ok(_) => {}
    }
    let manifest = read_backup(exe_dir)
        .map_err(|e| refuse(format!("{e:#}")))?
        .ok_or_else(|| refuse("the backup has no manifest".to_string()))?;
    let mut seen = BTreeSet::new();
    for name in manifest
        .saved
        .iter()
        .map(|s| &s.path)
        .chain(manifest.absent.iter())
    {
        check_name(name, allowed).map_err(&refuse)?;
        if !seen.insert(name) {
            return Err(refuse(format!("{name:?} is listed more than once")));
        }
    }
    for saved in &manifest.saved {
        match std::fs::symlink_metadata(dir.join(&saved.path)) {
            Ok(meta) if !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(refuse(format!("the saved {:?} is a link", saved.path))),
            Err(_) => return Err(refuse(format!("the saved {:?} is missing", saved.path))),
        }
    }
    Ok(Some(manifest))
}

fn put_back(exe_dir: &Path, manifest: &BackupManifest) -> Result<()> {
    let dir = exe_dir.join(BACKUP_DIR);
    let result = (|| -> Result<()> {
        for saved in &manifest.saved {
            let here = exe_dir.join(&saved.path);
            remove_any(&here).with_context(|| format!("cannot remove {}", here.display()))?;
            std::fs::rename(dir.join(&saved.path), &here)
                .with_context(|| format!("cannot put back {}", here.display()))?;
        }
        for name in &manifest.absent {
            let here = exe_dir.join(name);
            remove_any(&here).with_context(|| format!("cannot remove {}", here.display()))?;
        }
        std::fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))
    })();
    result.with_context(|| {
        format!(
            "{}: restore your files by hand from {}",
            dir.join(BACKUP_MANIFEST).display(),
            dir.display()
        )
    })
}

/// Put back what [`backup`] saved, remove what was not there before, and
/// remove [`BACKUP_DIR`]. `allowed` is the names the manifest's route writes;
/// the whole manifest is checked before anything is deleted.
pub fn restore(exe_dir: &Path, allowed: &BTreeSet<String>) -> Result<()> {
    match validated(exe_dir, allowed)? {
        Some(manifest) => put_back(exe_dir, &manifest),
        None => Ok(()),
    }
}

/// The last lines of the install log, for an error.
fn tail(log: &Path, lines: usize) -> String {
    let text = std::fs::read(log)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn run_script(package_dir: &Path, log: &Path, args: &[&std::ffi::OsStr]) -> Result<ExitStatus> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("{}", parent.display()))?;
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("{}", log.display()))?;
    let err = out.try_clone()?;
    let script = package_dir.join("install.sh");
    Command::new("bash")
        .arg(&script)
        .args(args)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .status()
        .with_context(|| format!("cannot run {}", script.display()))
}

/// Upstream's `install.sh <exe dir> <route> --model <M> --renodx
/// --no-lumenite`, non-interactive, its output appended to `log`. On failure
/// its partial files are removed again and the error ends with the log tail.
pub fn run_install(
    package_dir: &Path,
    exe_dir: &Path,
    route: Route,
    model: Network,
    log: &Path,
) -> Result<()> {
    let args = [
        exe_dir.as_os_str(),
        route.as_str().as_ref(),
        "--model".as_ref(),
        model.as_str().as_ref(),
        "--renodx".as_ref(),
        "--no-lumenite".as_ref(),
    ];
    let failure = match run_script(package_dir, log, &args) {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => format!("DLSSNR-AMD's install.sh failed ({status})"),
        Err(e) => format!("{e:#}"),
    };
    let removed = run_remove(package_dir, exe_dir, log);
    let mut message = failure;
    if let Err(e) = removed {
        message.push_str(&format!("; removing its files failed too: {e:#}"));
    }
    bail!(
        "{message}; the log ({}) ends with:\n{}",
        log.display(),
        tail(log, LOG_TAIL)
    )
}

/// Upstream's `install.sh <exe dir> remove`.
pub fn run_remove(package_dir: &Path, exe_dir: &Path, log: &Path) -> Result<()> {
    let status = run_script(package_dir, log, &[exe_dir.as_os_str(), "remove".as_ref()])?;
    if !status.success() {
        bail!(
            "DLSSNR-AMD's install.sh remove failed ({status}); the log ({}) ends with:\n{}",
            log.display(),
            tail(log, LOG_TAIL)
        );
    }
    Ok(())
}

/// Whether the install in `exe_dir` is whole: the install manifest is there,
/// everything it lists exists (logs aside), and the files the route loads
/// through are the package's.
pub fn verify(package_dir: &Path, exe_dir: &Path, route: Route) -> Result<(), String> {
    let manifest = exe_dir.join(INSTALL_MANIFEST);
    let text =
        std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let mut listed = BTreeSet::new();
    for line in text.lines() {
        let relative = line.trim().replace('\\', "/");
        if relative.is_empty() {
            continue;
        }
        let path = Path::new(&relative);
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(format!(
                "{} lists {relative:?}, which is not inside the game folder",
                manifest.display()
            ));
        }
        if let Some(Component::Normal(first)) = path.components().next() {
            listed.insert(first.to_string_lossy().into_owned());
        }
        if relative.ends_with(".log") {
            continue;
        }
        if std::fs::symlink_metadata(exe_dir.join(path)).is_err() {
            return Err(format!("{relative} is missing from {}", exe_dir.display()));
        }
    }
    let files = package::route_files(package_dir, route).map_err(|e| format!("{e:#}"))?;
    let injectors = package::injectors(route);
    let compared = injectors.required.iter().chain(
        injectors
            .when_listed
            .iter()
            .filter(|name| listed.contains(**name)),
    );
    for name in compared {
        let source = files
            .iter()
            .find(|f| f.name == *name)
            .and_then(|f| f.source.clone())
            .ok_or_else(|| format!("the package has no {name} for the {route} route"))?;
        if !same_bytes(&exe_dir.join(name), &source) {
            return Err(format!(
                "{name} in {} is not DLSSNR-AMD's",
                exe_dir.display()
            ));
        }
    }
    Ok(())
}

/// Back up, run `install.sh`, and keep the person's `dlssnr-amd.ini` from an
/// earlier install. On failure `install.sh`'s partial files are gone and the
/// person's ini is back; the backup is left for the caller.
fn install(
    ctx: &Context,
    key: &str,
    package_dir: &Path,
    exe_dir: &Path,
    entry: &Entry,
) -> Result<()> {
    let files = package::route_files(package_dir, entry.route)?;
    backup(exe_dir, &files, &entry.package, entry.route)?;
    let backup_dir = exe_dir.join(BACKUP_DIR);
    // The ini is the person's settings only when it was not there before the
    // first install (else it is saved as theirs and comes back on disable).
    let ini_is_ours = read_backup(exe_dir)?.is_some_and(|m| m.absent.iter().any(|a| a == INI));
    let ini = exe_dir.join(INI);
    let kept = backup_dir.join(KEPT_INI);
    if ini_is_ours && std::fs::symlink_metadata(&ini).is_ok_and(|m| m.is_file()) {
        std::fs::copy(&ini, &kept).with_context(|| format!("cannot keep {}", ini.display()))?;
    }
    let installed = run_install(
        package_dir,
        exe_dir,
        entry.route,
        entry.model,
        &log_file(ctx, key),
    );
    // Whether or not it worked: a failed run removed the ini with the rest.
    let kept_back = if kept.is_file() {
        std::fs::rename(&kept, &ini)
            .with_context(|| format!("cannot put back your {}", ini.display()))
    } else {
        Ok(())
    };
    installed?;
    kept_back?;
    record_installed(exe_dir, &files)
}

/// The model files a network needs.
fn needs(model: Network, expect: &Expect) -> &ModelFile {
    match model {
        Network::SmallFp8 | Network::SmallFp8Experimental => &expect.small,
        Network::SilverAardwold | Network::SilverAardwoldOptimized => &expect.aardwold,
    }
}

pub struct EnableRequest<'a> {
    /// The product id the state is kept under.
    pub key: &'a str,
    pub game_dir: &'a Path,
    /// Relative to `game_dir` (either separator) or absolute.
    pub executable: &'a str,
    /// Chosen by hand; detected when `None`.
    pub route: Option<Route>,
    pub model: Option<Network>,
    pub accept_anticheat_risk: bool,
}

#[derive(Debug, Clone)]
pub struct Enabled {
    pub entry: Entry,
    pub detected: Detected,
    pub warnings: Vec<String>,
}

/// Turn neural rendering on for a game: gate, route, package, model, backup,
/// install, and only then record it (with any acknowledgement), so a failure
/// at any step stores nothing.
pub fn enable(ctx: &Context, req: &EnableRequest) -> Result<Enabled> {
    let found = anticheat::detect(req.game_dir, req.executable);
    enable_with(ctx, req, &found)
}

fn enable_with(ctx: &Context, req: &EnableRequest, found: &[Detection]) -> Result<Enabled> {
    let _lock = lock(ctx, req.key)?;
    let name = game_name(req.game_dir, req.key);
    let acknowledged = state::acknowledged(&ctx.config_dir, req.key)?;
    let newly = match gate(found, &acknowledged) {
        Gate::Allowed => Vec::new(),
        Gate::Refused(names) => bail!(
            "{name} uses {}, a kernel-level anti-cheat; neural rendering is never enabled for it.",
            names.join(", ")
        ),
        Gate::NeedsAcknowledgement(names) if !req.accept_anticheat_risk => bail!(
            "{name} uses {}. Modding a game with anti-cheat can get the account banned; \
             to enable neural rendering anyway, pass --accept-anticheat-risk",
            names.join(", ")
        ),
        Gate::NeedsAcknowledgement(names) => names,
    };
    let dir = exe_dir(req.game_dir, req.executable);
    if !dir.is_dir() {
        bail!("{} is not a folder; is {name} installed?", dir.display());
    }
    if dir.join(INSTALL_MANIFEST).exists() && read_backup(&dir)?.is_none() {
        bail!(
            "DLSSNR-AMD in {} was installed by hand; remove it with its own install.sh \
             (install.sh \"{}\" remove) first",
            dir.display(),
            dir.display()
        );
    }
    let executable = PathBuf::from(req.executable.replace('\\', "/"));
    let detected = detect::detect(req.game_dir, &executable);
    let route = match req.route.or(detected.route) {
        Some(route) => route,
        None => bail!(
            "cannot tell which DLSSNR-AMD route suits {} ({}); pass --route with one of {}",
            req.executable,
            match detected.bits {
                0 => "its executable could not be read".to_string(),
                bits => format!("{bits}-bit, {:?}", detected.api),
            },
            Route::ALL.map(Route::as_str).join(", ")
        ),
    };
    if route == Route::Optiscaler && detected.bits == 32 {
        bail!(
            "the optiscaler route needs a 64-bit game; {} is 32-bit",
            req.executable
        );
    }
    // Refused before anything is written: one install per game, one route.
    if let Some(previous) = read_backup(&dir)? {
        if previous.route != route {
            bail!(
                "neural rendering is installed in {} with the {} route; turn it off before choosing another",
                dir.display(),
                previous.route
            );
        }
    }
    if let Some(old) = state::get(&ctx.config_dir, req.key)? {
        if old.install_dir != dir && holds_ours(&old.install_dir) {
            bail!(
                "neural rendering for {name} is installed in {}; turn it off before installing it in {}",
                old.install_dir.display(),
                dir.display()
            );
        }
    }
    // Only a backup this call starts is rolled back on failure: an earlier
    // one belongs to the install already there.
    let fresh = std::fs::symlink_metadata(dir.join(BACKUP_DIR)).is_err();
    let arch = if detected.bits == 32 {
        Arch::I686
    } else {
        Arch::X86_64
    };
    let package = (ctx.packages)(arch)?;
    let package_dir = package::ensure(&ctx.root, &package)?;
    if !package::expects_models(&package_dir, &ctx.expect) {
        bail!(
            "the DLSSNR-AMD {} package expects other model files than this Ferestre caches; \
             use the package this Ferestre pins",
            package.version
        );
    }
    let cached = models::verify(&ctx.root, &ctx.expect)?;
    if cached.is_empty() {
        bail!("no model cached; run: ferestre neural setup");
    }
    let has = |model: Network| cached.contains(needs(model, &ctx.expect));
    let model = match req.model {
        Some(model) if has(model) => model,
        Some(model) => bail!(
            "{model} needs {}, which is not cached (nvngx_dlssnr.dll 310.9.16 carries it); \
             run: ferestre neural setup --dll <its path>",
            needs(model, &ctx.expect).file
        ),
        None if has(Network::SmallFp8) => Network::SmallFp8,
        None => Network::SilverAardwold,
    };
    models::stage(&package_dir, &ctx.root, &ctx.expect)?;
    let entry = Entry {
        enabled: true,
        route,
        model,
        package: package.version.clone(),
        arch,
        install_dir: dir.clone(),
    };
    let allowed = names(&package::route_files(&package_dir, route)?);
    let undo = |error: anyhow::Error| -> anyhow::Error {
        if !fresh {
            return error.context(format!(
                "the earlier install in {} was not removed and your files are still kept in {}; \
                 the next launch installs it again, or turn neural rendering off",
                dir.display(),
                dir.join(BACKUP_DIR).display()
            ));
        }
        if let Err(e) = restore(&dir, &allowed) {
            return error.context(format!("{e:#}"));
        }
        error
    };
    if let Err(error) = install(ctx, req.key, &package_dir, &dir, &entry) {
        return Err(undo(error));
    }
    if let Err(error) = state::record(&ctx.config_dir, req.key, &entry, &newly) {
        if fresh {
            let _ = run_remove(&package_dir, &dir, &log_file(ctx, req.key));
        }
        return Err(undo(error));
    }
    let mut warnings = hardware::check();
    if detected.assumed {
        warnings.push(format!(
            "{} could not be read (an Xbox install keeps it encrypted); assumed x86-64 D3D12",
            req.executable
        ));
    }
    Ok(Enabled {
        entry,
        detected,
        warnings,
    })
}

/// Turn neural rendering off for a game: `install.sh remove`, the person's
/// files back, the entry forgotten. Any error leaves the entry in place.
pub fn disable(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Result<()> {
    let _lock = lock(ctx, key)?;
    disable_locked(ctx, key, game_dir, executable)
}

fn disable_locked(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Result<()> {
    let entry = state::get(&ctx.config_dir, key)?;
    let dir = entry
        .as_ref()
        .map(|e| e.install_dir.clone())
        .unwrap_or_else(|| exe_dir(game_dir, executable));
    take_out(ctx, key, &dir, entry.as_ref())?;
    state::forget(&ctx.config_dir, key)
}

/// Whether a folder holds any of Ferestre's install or backup.
fn holds_ours(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(INSTALL_MANIFEST)).is_ok()
        || std::fs::symlink_metadata(dir.join(BACKUP_DIR)).is_ok()
}

/// `install.sh remove` in `dir` and the person's files back, leaving the
/// state alone. `entry` names the package and route; without one the backup
/// manifest does.
fn take_out(ctx: &Context, key: &str, dir: &Path, entry: Option<&Entry>) -> Result<()> {
    if !dir.is_dir() || !holds_ours(dir) {
        return Ok(());
    }
    let (version, arches, route) = match entry {
        Some(e) => (e.package.clone(), vec![e.arch], e.route),
        None => match read_backup(dir)? {
            Some(m) => (m.package, Arch::ALL.to_vec(), m.route),
            None => return Ok(()),
        },
    };
    let package_dir = arches
        .iter()
        .find_map(|&arch| package::unpacked(&ctx.root, &version, arch))
        .ok_or_else(|| {
            anyhow!(
                "the DLSSNR-AMD {version} package that installed neural rendering in {} is gone; \
                 run: ferestre neural setup, then turn it off again",
                dir.display()
            )
        })?;
    let allowed = names(&package::route_files(&package_dir, route)?);
    // Checked whole before `install.sh remove` deletes anything.
    let backup = validated(dir, &allowed)?;
    if dir.join(INSTALL_MANIFEST).exists() {
        run_remove(&package_dir, dir, &log_file(ctx, key))?;
    }
    if let Some(manifest) = backup {
        put_back(dir, &manifest)?;
    }
    Ok(())
}

/// What [`ensure_installed`] found or did before a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    NotEnabled,
    /// Installed and whole; add the launch env.
    Present(Entry),
    /// Installed again (a game update or file check removed it); add the
    /// launch env.
    Reinstalled(Entry),
    /// Off for this launch, nothing of the mod in the game folder.
    Skipped(String),
    /// An anti-cheat now forbids it: taken out, the person's files back, the
    /// entry forgotten.
    TakenOut(String),
    /// An anti-cheat now forbids it and its files could not be taken out:
    /// the launch must stop.
    Blocked(String),
}

/// Before every launch: re-run the anti-cheat gate, then make sure an
/// enabled install is whole, reinstalling it when it is not.
pub fn ensure_installed(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Ensured {
    let found = anticheat::detect(game_dir, executable);
    ensure_with(ctx, key, game_dir, executable, &found)
}

fn ensure_with(
    ctx: &Context,
    key: &str,
    game_dir: &Path,
    executable: &str,
    found: &[Detection],
) -> Ensured {
    let _lock = match lock(ctx, key) {
        Ok(lock) => lock,
        Err(e) => return Ensured::Skipped(format!("{e:#}")),
    };
    let entry = match state::get(&ctx.config_dir, key) {
        Ok(Some(entry)) if entry.enabled => entry,
        Ok(_) => return Ensured::NotEnabled,
        Err(e) => return Ensured::Skipped(format!("{e:#}")),
    };
    let acknowledged = match state::acknowledged(&ctx.config_dir, key) {
        Ok(acknowledged) => acknowledged,
        Err(e) => return Ensured::Skipped(format!("{e:#}")),
    };
    let dir = exe_dir(game_dir, executable);
    if let Some(reason) = gate_reason(&gate(found, &acknowledged)) {
        if !holds_ours(&dir) && !holds_ours(&entry.install_dir) {
            return Ensured::Skipped(reason);
        }
        return match disable_locked(ctx, key, game_dir, executable) {
            Ok(()) => Ensured::TakenOut(format!(
                "{reason}; its files were taken out and yours put back"
            )),
            Err(e) => Ensured::Blocked(format!(
                "{reason}, and its files could not be taken out: {e:#}"
            )),
        };
    }
    let current = (ctx.packages)(entry.arch);
    let same_package = current
        .as_ref()
        .map_or(true, |p| p.version == entry.package);
    let whole = entry.install_dir == dir
        && same_package
        && package::unpacked(&ctx.root, &entry.package, entry.arch)
            .is_some_and(|pkg| verify(&pkg, &dir, entry.route).is_ok());
    if whole {
        return Ensured::Present(entry);
    }
    // The executable moved to another folder: the old install comes out
    // first, so neither it nor the person's files kept beside it are lost.
    if entry.install_dir != dir && holds_ours(&entry.install_dir) {
        if let Err(e) = take_out(ctx, key, &entry.install_dir, Some(&entry)) {
            return Ensured::Skipped(format!(
                "the executable is now in {}, and neural rendering's earlier install in {} \
                 could not be taken out: {e:#}",
                dir.display(),
                entry.install_dir.display()
            ));
        }
    }
    match reinstall(ctx, key, &dir, &entry, current) {
        Ok(entry) => Ensured::Reinstalled(entry),
        Err(e) => Ensured::Skipped(format!("{e:#}")),
    }
}

fn reinstall(
    ctx: &Context,
    key: &str,
    dir: &Path,
    entry: &Entry,
    current: Result<package::Package>,
) -> Result<Entry> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let package = current?;
    let package_dir = package::ensure(&ctx.root, &package)?;
    if !package::expects_models(&package_dir, &ctx.expect) {
        bail!(
            "the DLSSNR-AMD {} package expects other model files than this Ferestre caches",
            package.version
        );
    }
    let model = needs(entry.model, &ctx.expect);
    if !models::verify(&ctx.root, &ctx.expect)?.contains(model) {
        bail!("{} is not cached; run: ferestre neural setup", model.file);
    }
    models::stage(&package_dir, &ctx.root, &ctx.expect)?;
    let entry = Entry {
        package: package.version.clone(),
        install_dir: dir.to_path_buf(),
        ..entry.clone()
    };
    install(ctx, key, &package_dir, dir, &entry)?;
    state::put(&ctx.config_dir, key, &entry)?;
    Ok(entry)
}

/// The enabled entry when the anti-cheat gate allows it, reading only: what
/// a dry run shows a launch would add.
pub fn preview(ctx: &Context, key: &str, game_dir: &Path, executable: &str) -> Option<Entry> {
    let entry = state::get(&ctx.config_dir, key).ok()??;
    if !entry.enabled {
        return None;
    }
    let acknowledged = state::acknowledged(&ctx.config_dir, key).ok()?;
    let found = anticheat::detect(game_dir, executable);
    (gate(&found, &acknowledged) == Gate::Allowed).then_some(entry)
}

/// The DLL overrides a route loads through.
pub fn dll_overrides(route: Route) -> &'static [(&'static str, &'static str)] {
    match route {
        Route::Optiscaler | Route::ReshadeFeeder => &[("dxgi", "n,b")],
        Route::ReshadeFeederVulkan | Route::ReshadeFeederDx9 => {
            &[("winevulkan", "n,b"), ("vulkan-1", "n,b")]
        }
    }
}

fn dll_name(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    name.strip_suffix(".dll").unwrap_or(&name).to_string()
}

/// `existing` with ours appended for every DLL it does not name already
/// (the person's own setting wins), and the names it already set.
pub fn merge_dll_overrides(existing: Option<&str>, ours: &[(&str, &str)]) -> (String, Vec<String>) {
    let existing = existing.unwrap_or("");
    let specified: BTreeSet<String> = existing
        .split(';')
        .flat_map(|group| group.split('=').next().unwrap_or("").split(','))
        .map(dll_name)
        .filter(|n| !n.is_empty())
        .collect();
    let mut groups = Vec::new();
    if !existing.is_empty() {
        groups.push(existing.to_string());
    }
    let mut theirs = Vec::new();
    for (name, mode) in ours {
        if specified.contains(&dll_name(name)) {
            theirs.push(name.to_string());
        } else {
            groups.push(format!("{name}={mode}"));
        }
    }
    (groups.join(";"), theirs)
}

/// What a launch adds for neural rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchEnv {
    pub set: Vec<(String, String)>,
    pub notes: Vec<String>,
}

/// `value` with `item` appended to its `:`-list unless it is there already.
fn append_path_list(value: Option<String>, item: &str) -> String {
    match value {
        Some(v) if v.split(':').any(|part| part == item) => v,
        Some(v) if !v.is_empty() => format!("{v}:{item}"),
        _ => item.to_string(),
    }
}

/// The launch env for an installed entry. `current` is the value a key
/// would have at launch without neural rendering.
pub fn launch_env(
    entry: &Entry,
    exe_dir: &Path,
    current: &dyn Fn(&str) -> Option<String>,
) -> LaunchEnv {
    let mut env = LaunchEnv::default();
    let existing = current("WINEDLLOVERRIDES");
    let (merged, theirs) = merge_dll_overrides(existing.as_deref(), dll_overrides(entry.route));
    env.set.push(("WINEDLLOVERRIDES".to_string(), merged));
    for name in theirs {
        env.notes.push(format!(
            "-- neural rendering: your WINEDLLOVERRIDES sets {name} itself, so DLSSNR-AMD may not load"
        ));
    }
    if entry.route != Route::Optiscaler
        && entry.model == Network::SilverAardwoldOptimized
        && exe_dir.join(INT4_MODEL).is_file()
    {
        let layer = exe_dir.join(INT4_LAYER_DIR).display().to_string();
        env.set.push((
            "VK_ADD_LAYER_PATH".to_string(),
            append_path_list(current("VK_ADD_LAYER_PATH"), &layer),
        ));
        env.set.push((
            "VK_INSTANCE_LAYERS".to_string(),
            append_path_list(current("VK_INSTANCE_LAYERS"), INT4_LAYER),
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::dll::tests::expect_with;
    use crate::neural::overview;
    use crate::neural::package;
    use std::process::Command;

    const THEIRS: &[u8] = b"theirs";

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Emulates upstream `install.sh` at the pin: `remove` deletes what the
    /// manifest lists; a route first removes a previous install, writes the
    /// manifest, then the files, and fails after writing when the package
    /// has a `fail-with` file (as `patch_ini.py` would, part way).
    const INSTALL_SH: &str = r#"#!/bin/bash
set -euo pipefail
model_sha256=@AARDWOLD@
small_sha256=@SMALL@
pkg="$(cd "$(dirname "$0")" && pwd)"
dir="$1"
route="$2"
model="${4:-}"
echo "install.sh $route $model"
manifest="$dir/dlssnr-amd-install.txt"
remove() {
  if [ -f "$manifest" ]; then
    while IFS= read -r p; do
      [ -n "$p" ] && rm -rf "${dir:?}/$p"
    done < "$manifest"
    rm -f "$manifest"
  fi
}
if [ "$route" = remove ]; then remove; echo removed; exit 0; fi
remove
pairs=("$pkg/dlssnr-amd|dlssnr-amd")
case "$route" in
  reshade-feeder)
    for f in "$pkg"/reshade/*; do pairs+=("$f|$(basename "$f")"); done ;;
  reshade-feeder-vulkan|reshade-feeder-dx9)
    for f in "$pkg"/reshade/* "$pkg"/vulkan/*; do
      n="$(basename "$f")"
      [ "$n" = dxgi.dll ] && continue
      pairs+=("$f|$n")
    done ;;
  optiscaler)
    pairs+=("$pkg/optiscaler/release/OptiScaler.dll|dxgi.dll")
    pairs+=("$pkg/optiscaler/release/OptiScaler.ini|OptiScaler.ini")
    pairs+=("$pkg/optiscaler/release/OptiScaler|OptiScaler")
    [ -e "$dir/vulkan-1.dll" ] || pairs+=("$pkg/optiscaler/vulkan-1.dll|vulkan-1.dll") ;;
  *) echo "unknown route $route" >&2; exit 1 ;;
esac
{ for p in "${pairs[@]}"; do echo "${p#*|}"; done; echo dlssnr-amd.ini; } > "$manifest"
for p in "${pairs[@]}"; do
  src="${p%%|*}"; dst="${p#*|}"
  if [ -d "$src" ]; then mkdir -p "$dir/$dst"; cp -r "$src/." "$dir/$dst/"; else cp "$src" "$dir/$dst"; fi
done
printf 'default\n' > "$dir/dlssnr-amd.ini"
if [ "$model" = CC_SILVER_AARDWOLD_OPTIMIZED ]; then
  printf int4 > "$dir/dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin"
fi
echo "installed $route"
exit "$(cat "$pkg/fail-with" 2>/dev/null || echo 0)"
"#;

    /// A fake package tarball in `dir` (the [`package::LOCAL_ENV`] layout);
    /// returns `dir`.
    fn fake_package(dir: &Path, expect: &Expect, fail_with: Option<u8>) -> PathBuf {
        let src = tempfile::tempdir().unwrap();
        let p = src.path();
        let script = INSTALL_SH
            .replace("@AARDWOLD@", &expect.aardwold.sha256)
            .replace("@SMALL@", &expect.small.sha256);
        write(&p.join("install.sh"), script.as_bytes());
        write(&p.join("model-tools/extract_model.sh"), b"#!/bin/bash\n");
        write(&p.join("dlssnr-amd/shader.spv"), b"spv");
        write(&p.join("reshade/dxgi.dll"), b"reshade dxgi");
        write(&p.join("reshade/ReShade.ini"), b"package ini");
        write(&p.join("reshade/reshade-shaders/x.fx"), b"fx");
        write(&p.join("vulkan/vulkan-1.dll"), b"loader");
        write(&p.join("vulkan/winevulkan.dll"), b"winevulkan");
        write(&p.join("optiscaler/release/OptiScaler.dll"), b"opti");
        write(&p.join("optiscaler/release/OptiScaler.ini"), b"opti ini");
        write(&p.join("optiscaler/release/OptiScaler/x"), b"x");
        write(&p.join("optiscaler/vulkan-1.dll"), b"forwarder");
        if let Some(code) = fail_with {
            write(&p.join("fail-with"), code.to_string().as_bytes());
        }
        std::fs::create_dir_all(dir).unwrap();
        let ok = Command::new("tar")
            .arg("-czf")
            .arg(dir.join(package::asset_name("0.0.9", Arch::X86_64)))
            .arg("-C")
            .arg(p)
            .arg(".")
            .status()
            .unwrap()
            .success();
        assert!(ok);
        dir.to_path_buf()
    }

    struct World {
        tmp: tempfile::TempDir,
        game: PathBuf,
        ctx: Context,
    }

    impl World {
        fn new() -> World {
            World::with(None)
        }

        fn with(fail_with: Option<u8>) -> World {
            let tmp = tempfile::tempdir().unwrap();
            let expect = expect_with(b"aardwold", b"small");
            let packages = fake_package(&tmp.path().join("pkgs"), &expect, fail_with);
            let root = tmp.path().join("state/dlssnr");
            write(&root.join("models/CC_SILVER_AARDWOLD.bin"), b"aardwold");
            write(&root.join("models/CC_SMALL_FP8.bin"), b"small");
            let game = tmp.path().join("games/Game");
            write(
                &game.join("Game.exe"),
                &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["d3d12.dll"]),
            );
            let ctx = Context {
                root,
                config_dir: tmp.path().join("config"),
                packages: Box::new(move |arch| package::local(&packages, arch)),
                expect,
            };
            World { tmp, game, ctx }
        }

        fn req(&self, route: Option<Route>, accept: bool) -> EnableRequest<'_> {
            EnableRequest {
                key: "local-game",
                game_dir: &self.game,
                executable: "Game.exe",
                route,
                model: None,
                accept_anticheat_risk: accept,
            }
        }

        fn enable(&self, route: Route) -> Enabled {
            enable_with(&self.ctx, &self.req(Some(route), false), &[]).unwrap()
        }

        fn entry(&self) -> Option<Entry> {
            state::get(&self.ctx.config_dir, "local-game").unwrap()
        }

        fn ensure(&self, found: &[Detection]) -> Ensured {
            ensure_with(&self.ctx, "local-game", &self.game, "Game.exe", found)
        }

        fn log_lines(&self) -> usize {
            std::fs::read_to_string(self.ctx.root.join("logs/LOCAL-GAME.log"))
                .map(|t| t.lines().count())
                .unwrap_or(0)
        }

        fn package_dir(&self) -> PathBuf {
            self.ctx.root.join("packages/0.0.9-x86_64")
        }
    }

    fn det(name: &'static str, kernel: bool, proton_runtime: bool) -> Detection {
        Detection {
            name,
            kernel,
            proton_runtime,
            evidence: PathBuf::new(),
        }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_gate_refuses_kernel_anti_cheats_proton_cannot_run() {
        let none = BTreeSet::new();
        assert_eq!(gate(&[], &none), Gate::Allowed);
        assert_eq!(
            gate(
                &[
                    det("Easy Anti-Cheat", true, true),
                    det("Ricochet", true, false)
                ],
                &none
            ),
            Gate::Refused(vec!["Ricochet"])
        );
        assert_eq!(
            gate(&[det("BattlEye", true, true)], &none),
            Gate::NeedsAcknowledgement(vec!["BattlEye"])
        );
        let acknowledged = BTreeSet::from(["BattlEye".to_string()]);
        assert_eq!(
            gate(&[det("BattlEye", true, true)], &acknowledged),
            Gate::Allowed
        );
    }

    #[test]
    fn a_kernel_anti_cheat_refuses_even_when_risk_is_accepted() {
        let w = World::new();
        let before = listing(&w.game);
        let error = enable_with(
            &w.ctx,
            &w.req(Some(Route::ReshadeFeeder), true),
            &[det("Ricochet", true, false)],
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("Ricochet"), "{text}");
        assert!(text.contains("kernel-level"), "{text}");
        assert_eq!(listing(&w.game), before, "no file written");
        assert_eq!(w.entry(), None);
        assert!(state::acknowledged(&w.ctx.config_dir, "local-game")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn another_anti_cheat_needs_an_acknowledgement_that_is_remembered() {
        let w = World::new();
        let eac = [det("Easy Anti-Cheat", true, true)];
        let error = enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), false), &eac)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--accept-anticheat-risk"), "{error}");
        assert!(error.contains("Easy Anti-Cheat"), "{error}");
        assert!(!w.game.join(INSTALL_MANIFEST).exists());

        enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), true), &eac).unwrap();
        assert!(state::acknowledged(&w.ctx.config_dir, "local-game")
            .unwrap()
            .contains("Easy Anti-Cheat"));
        enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), false), &eac).unwrap();
        assert!(w.entry().unwrap().enabled);
    }

    #[test]
    fn another_mods_files_come_back_byte_identical_after_disable() {
        let w = World::new();
        write(&w.game.join("dxgi.dll"), THEIRS);
        write(&w.game.join("ReShade.ini"), b"their ini");
        write(&w.game.join("reshade-shaders/a.fx"), b"their fx");
        let enabled = w.enable(Route::ReshadeFeeder);
        assert_eq!(enabled.entry.route, Route::ReshadeFeeder);
        assert_eq!(enabled.entry.install_dir, w.game);
        assert_eq!(read(&w.game.join("dxgi.dll")), b"reshade dxgi");
        assert!(w.game.join("reshade-shaders/x.fx").is_file());
        assert!(verify(&w.package_dir(), &w.game, Route::ReshadeFeeder).is_ok());

        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
        assert_eq!(read(&w.game.join("ReShade.ini")), b"their ini");
        assert_eq!(read(&w.game.join("reshade-shaders/a.fx")), b"their fx");
        assert!(!w.game.join("reshade-shaders/x.fx").exists());
        assert!(!w.game.join(BACKUP_DIR).exists());
        assert!(!w.game.join(INSTALL_MANIFEST).exists());
        assert_eq!(
            listing(&w.game),
            vec!["Game.exe", "ReShade.ini", "dxgi.dll", "reshade-shaders"]
        );
        assert_eq!(w.entry(), None);
    }

    #[test]
    fn a_failed_install_is_rolled_back_and_not_recorded() {
        let w = World::with(Some(1));
        write(&w.game.join("dxgi.dll"), THEIRS);
        let error = enable_with(
            &w.ctx,
            &w.req(Some(Route::ReshadeFeeder), true),
            &[det("Easy Anti-Cheat", true, true)],
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("installed reshade-feeder"),
            "log tail: {text}"
        );
        assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
        assert!(!w.game.join(INSTALL_MANIFEST).exists());
        assert!(!w.game.join(BACKUP_DIR).exists());
        assert_eq!(listing(&w.game), vec!["Game.exe", "dxgi.dll"]);
        assert_eq!(w.entry(), None);
        assert!(state::acknowledged(&w.ctx.config_dir, "local-game")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_tampered_backup_manifest_deletes_nothing() {
        let w = World::new();
        write(&w.game.join("dxgi.dll"), THEIRS);
        let canary = w.game.parent().unwrap().join("canary");
        write(&canary, b"alive");
        w.enable(Route::ReshadeFeeder);
        let manifest = w.game.join(BACKUP_DIR).join("manifest.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        let json: serde_json::Value = serde_json::from_str(&original).unwrap();
        assert_eq!(json["saved"][0]["path"], "dxgi.dll");
        assert!(json["absent"].as_array().unwrap().len() > 1);
        let mut tampered = Vec::new();
        for bad in ["..", "/tmp/x", "", "a/b", ".", "Game.exe", BACKUP_DIR] {
            let mut saved = json.clone();
            saved["saved"][0]["path"] = serde_json::Value::String(bad.to_string());
            tampered.push((format!("saved {bad:?}"), saved));
            let mut absent = json.clone();
            absent["absent"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::Value::String(bad.to_string()));
            tampered.push((format!("absent {bad:?}"), absent));
        }
        // A name both saved and absent would be put back, then deleted.
        let mut both = json.clone();
        both["absent"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::Value::String("dxgi.dll".into()));
        tampered.push(("saved and absent".into(), both));
        let mut twice = json.clone();
        let first = twice["absent"][0].clone();
        twice["absent"].as_array_mut().unwrap().push(first);
        tampered.push(("absent twice".into(), twice));
        for (bad, json) in tampered {
            std::fs::write(&manifest, json.to_string()).unwrap();
            let before = listing(&w.game);
            let error = disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap_err();
            assert!(
                format!("{error:#}").contains(BACKUP_DIR),
                "{bad}: {error:#}"
            );
            assert_eq!(listing(&w.game), before, "{bad}");
            assert!(w.game.join(BACKUP_DIR).is_dir(), "{bad}");
            assert_eq!(read(&canary), b"alive");
            assert_eq!(read(&w.game.join(BACKUP_DIR).join("dxgi.dll")), THEIRS);
            assert!(w.entry().is_some(), "{bad}: the entry stays");
            assert!(
                w.game.join(INSTALL_MANIFEST).is_file(),
                "{bad}: not removed"
            );
            std::fs::write(&manifest, &original).unwrap();
        }
        // A saved copy that is a symlink is refused too.
        let copy = w.game.join(BACKUP_DIR).join("dxgi.dll");
        std::fs::remove_file(&copy).unwrap();
        std::os::unix::fs::symlink(&canary, &copy).unwrap();
        let before = listing(&w.game);
        let backup_before = listing(&w.game.join(BACKUP_DIR));
        assert!(disable(&w.ctx, "local-game", &w.game, "Game.exe").is_err());
        assert_eq!(read(&canary), b"alive");
        assert_eq!(listing(&w.game), before);
        assert_eq!(listing(&w.game.join(BACKUP_DIR)), backup_before);
        assert!(w.game.join(INSTALL_MANIFEST).is_file());
        assert!(w.entry().is_some());
    }

    #[test]
    fn choosing_another_route_is_refused_without_changes() {
        let w = World::new();
        write(&w.game.join("winevulkan.dll"), THEIRS);
        w.enable(Route::ReshadeFeederVulkan);
        write(&w.game.join(INI), b"mine");
        let before = listing(&w.game);
        let backup_before = listing(&w.game.join(BACKUP_DIR));
        let lines = w.log_lines();
        let error = enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeederDx9), false), &[])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("turn it off before choosing another"),
            "{error}"
        );
        assert_eq!(listing(&w.game), before);
        assert_eq!(listing(&w.game.join(BACKUP_DIR)), backup_before);
        assert_eq!(read(&w.game.join(INI)), b"mine");
        assert_eq!(w.log_lines(), lines, "install.sh did not run");
        assert_eq!(w.entry().unwrap().route, Route::ReshadeFeederVulkan);
        assert!(verify(&w.package_dir(), &w.game, Route::ReshadeFeederVulkan).is_ok());
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(listing(&w.game), vec!["Game.exe", "winevulkan.dll"]);
        assert_eq!(read(&w.game.join("winevulkan.dll")), THEIRS);
    }

    #[test]
    fn a_failed_reenable_keeps_the_backup_and_the_persons_ini() {
        let w = World::new();
        write(&w.game.join("dxgi.dll"), THEIRS);
        w.enable(Route::ReshadeFeeder);
        write(&w.game.join(INI), b"mine");
        let fail = w.package_dir().join("fail-with");
        write(&fail, b"1");
        let error =
            enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), false), &[]).unwrap_err();
        assert!(
            format!("{error:#}").contains("was not removed"),
            "{error:#}"
        );
        assert_eq!(read(&w.game.join(INI)), b"mine");
        assert_eq!(read(&w.game.join(BACKUP_DIR).join("dxgi.dll")), THEIRS);
        assert!(!w.game.join(BACKUP_DIR).join(KEPT_INI).exists());
        assert!(w.entry().unwrap().enabled);

        std::fs::remove_file(&fail).unwrap();
        assert!(matches!(w.ensure(&[]), Ensured::Reinstalled(_)));
        assert_eq!(read(&w.game.join(INI)), b"mine");
        assert_eq!(read(&w.game.join("dxgi.dll")), b"reshade dxgi");
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(listing(&w.game), vec!["Game.exe", "dxgi.dll"]);
        assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
    }

    #[test]
    fn a_changed_package_with_a_wiped_manifest_never_saves_our_old_files() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        std::fs::remove_file(w.game.join(INSTALL_MANIFEST)).unwrap();
        write(
            &w.package_dir().join("reshade/dxgi.dll"),
            b"newer reshade dxgi",
        );
        match w.ensure(&[]) {
            Ensured::Reinstalled(entry) => assert_eq!(entry.route, Route::ReshadeFeeder),
            other => panic!("{other:?}"),
        }
        assert!(!w.game.join(BACKUP_DIR).join("dxgi.dll").exists());
        assert_eq!(read(&w.game.join("dxgi.dll")), b"newer reshade dxgi");
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(listing(&w.game), vec!["Game.exe"]);
    }

    #[test]
    fn a_moved_executable_takes_the_old_install_out_first() {
        let w = World::new();
        write(&w.game.join("dxgi.dll"), THEIRS);
        w.enable(Route::ReshadeFeeder);
        let bin = w.game.join("Bin");
        write(
            &bin.join("Game.exe"),
            &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["d3d12.dll"]),
        );
        // Enable refuses a second folder while the first holds the mod.
        let req = EnableRequest {
            executable: "Bin/Game.exe",
            ..w.req(Some(Route::ReshadeFeeder), false)
        };
        let error = enable_with(&w.ctx, &req, &[]).unwrap_err().to_string();
        assert!(error.contains("turn it off"), "{error}");
        assert_eq!(listing(&bin), vec!["Game.exe"]);

        match ensure_with(&w.ctx, "local-game", &w.game, "Bin/Game.exe", &[]) {
            Ensured::Reinstalled(entry) => assert_eq!(entry.install_dir, bin),
            other => panic!("{other:?}"),
        }
        assert_eq!(listing(&w.game), vec!["Bin", "Game.exe", "dxgi.dll"]);
        assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
        assert!(verify(&w.package_dir(), &bin, Route::ReshadeFeeder).is_ok());
        disable(&w.ctx, "local-game", &w.game, "Bin/Game.exe").unwrap();
        assert_eq!(listing(&bin), vec!["Game.exe"]);
        assert_eq!(w.entry(), None);
    }

    #[test]
    fn a_moved_executable_whose_old_install_cannot_come_out_is_skipped() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        let mut entry = w.entry().unwrap();
        entry.package = "0.0.8".into();
        state::put(&w.ctx.config_dir, "local-game", &entry).unwrap();
        let bin = w.game.join("Bin");
        write(
            &bin.join("Game.exe"),
            &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["d3d12.dll"]),
        );
        let before = listing(&w.game);
        match ensure_with(&w.ctx, "local-game", &w.game, "Bin/Game.exe", &[]) {
            Ensured::Skipped(reason) => assert!(reason.contains("taken out"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(listing(&w.game), before);
        assert_eq!(listing(&bin), vec!["Game.exe"]);
        assert_eq!(w.entry().unwrap().install_dir, w.game);
    }

    #[test]
    fn an_absolute_executable_installs_beside_itself() {
        let w = World::new();
        let elsewhere = w.tmp.path().join("elsewhere");
        write(
            &elsewhere.join("Game.exe"),
            &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["d3d11.dll"]),
        );
        let executable = elsewhere.join("Game.exe").display().to_string();
        assert_eq!(exe_dir(&w.game, &executable), elsewhere);
        assert_eq!(
            exe_dir(Path::new("/g"), r"Bin\Win64\Game.exe"),
            PathBuf::from("/g/Bin/Win64")
        );
        let before = listing(&w.game);
        let req = EnableRequest {
            executable: &executable,
            ..w.req(None, false)
        };
        let enabled = enable_with(&w.ctx, &req, &[]).unwrap();
        assert_eq!(enabled.entry.route, Route::ReshadeFeeder, "detected");
        assert_eq!(enabled.entry.install_dir, elsewhere);
        assert!(elsewhere.join(INSTALL_MANIFEST).is_file());
        assert_eq!(listing(&w.game), before);
    }

    #[test]
    fn optiscaler_verifies_vulkan_1_only_when_it_installed_it() {
        let w = World::new();
        write(&w.game.join("vulkan-1.dll"), b"the game's loader");
        w.enable(Route::Optiscaler);
        let listed = std::fs::read_to_string(w.game.join(INSTALL_MANIFEST)).unwrap();
        assert!(!listed.lines().any(|l| l == "vulkan-1.dll"));
        assert!(verify(&w.package_dir(), &w.game, Route::Optiscaler).is_ok());
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(read(&w.game.join("vulkan-1.dll")), b"the game's loader");

        std::fs::remove_file(w.game.join("vulkan-1.dll")).unwrap();
        w.enable(Route::Optiscaler);
        assert_eq!(read(&w.game.join("vulkan-1.dll")), b"forwarder");
        assert!(verify(&w.package_dir(), &w.game, Route::Optiscaler).is_ok());
        write(&w.game.join("vulkan-1.dll"), b"corrupt");
        let error = verify(&w.package_dir(), &w.game, Route::Optiscaler).unwrap_err();
        assert!(error.contains("vulkan-1.dll"), "{error}");
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert!(!w.game.join("vulkan-1.dll").exists());
    }

    #[test]
    fn optiscaler_refuses_a_32_bit_executable() {
        let w = World::new();
        write(
            &w.game.join("Game.exe"),
            &crate::neural::pe::tiny_pe(0x14c, 0x10b, &["d3d11.dll"]),
        );
        let error = enable_with(&w.ctx, &w.req(Some(Route::Optiscaler), false), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("32-bit"), "{error}");
        assert_eq!(w.entry(), None);
    }

    #[test]
    fn an_unknown_api_asks_for_a_route() {
        let w = World::new();
        write(
            &w.game.join("Game.exe"),
            &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["opengl32.dll"]),
        );
        let error = enable_with(&w.ctx, &w.req(None, false), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("--route"), "{error}");
    }

    #[test]
    fn a_game_folder_with_spaces_and_unicode_works() {
        let w = World::new();
        let game = w.tmp.path().join("Mé Game");
        write(
            &game.join("Bin 64/Game.exe"),
            &crate::neural::pe::tiny_pe(0x8664, 0x20b, &["vulkan-1.dll"]),
        );
        write(&game.join("Bin 64/winevulkan.dll"), THEIRS);
        let req = EnableRequest {
            key: "local-game",
            game_dir: &game,
            executable: r"Bin 64\Game.exe",
            route: None,
            model: None,
            accept_anticheat_risk: false,
        };
        let enabled = enable_with(&w.ctx, &req, &[]).unwrap();
        let bin = game.join("Bin 64");
        assert_eq!(enabled.entry.route, Route::ReshadeFeederVulkan);
        assert_eq!(enabled.entry.install_dir, bin);
        assert!(verify(&w.package_dir(), &bin, Route::ReshadeFeederVulkan).is_ok());
        disable(&w.ctx, "local-game", &game, r"Bin 64\Game.exe").unwrap();
        assert_eq!(read(&bin.join("winevulkan.dll")), THEIRS);
        assert_eq!(listing(&bin), vec!["Game.exe", "winevulkan.dll"]);
    }

    #[test]
    fn a_hand_made_install_is_refused() {
        let w = World::new();
        write(&w.game.join(INSTALL_MANIFEST), b"dxgi.dll\n");
        let error = enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), false), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("installed by hand"), "{error}");
        assert!(!w.game.join(BACKUP_DIR).exists());
    }

    #[test]
    fn a_model_that_is_not_cached_is_refused() {
        let w = World::new();
        std::fs::remove_file(w.ctx.root.join("models/CC_SMALL_FP8.bin")).unwrap();
        let req = EnableRequest {
            model: Some(Network::SmallFp8Experimental),
            ..w.req(Some(Route::ReshadeFeeder), false)
        };
        assert!(enable_with(&w.ctx, &req, &[]).is_err());
        let enabled = w.enable(Route::ReshadeFeeder);
        assert_eq!(enabled.entry.model, Network::SilverAardwold, "the default");
        std::fs::remove_file(w.ctx.root.join("models/CC_SILVER_AARDWOLD.bin")).unwrap();
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        let error = enable_with(&w.ctx, &w.req(Some(Route::ReshadeFeeder), false), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("ferestre neural setup"), "{error}");
    }

    #[test]
    fn a_wiped_install_is_redone_without_backing_up_our_own_files() {
        let w = World::new();
        write(&w.game.join("dxgi.dll"), THEIRS);
        w.enable(Route::ReshadeFeeder);
        std::fs::remove_dir_all(w.game.join("dlssnr-amd")).unwrap();
        std::fs::remove_file(w.game.join(INSTALL_MANIFEST)).unwrap();
        match w.ensure(&[]) {
            Ensured::Reinstalled(entry) => assert_eq!(entry.route, Route::ReshadeFeeder),
            other => panic!("{other:?}"),
        }
        assert!(w.game.join(INSTALL_MANIFEST).is_file());
        assert_eq!(read(&w.game.join(BACKUP_DIR).join("dxgi.dll")), THEIRS);
        assert!(!w.game.join(BACKUP_DIR).join("dlssnr-amd.ini").exists());
        disable(&w.ctx, "local-game", &w.game, "Game.exe").unwrap();
        assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
        assert_eq!(listing(&w.game), vec!["Game.exe", "dxgi.dll"]);
    }

    #[test]
    fn the_persons_ini_survives_a_reinstall() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        write(&w.game.join("dlssnr-amd.ini"), b"mine");
        write(&w.game.join("dxgi.dll"), b"replaced by a game update");
        assert!(matches!(w.ensure(&[]), Ensured::Reinstalled(_)));
        assert_eq!(read(&w.game.join("dlssnr-amd.ini")), b"mine");
        assert_eq!(read(&w.game.join("dxgi.dll")), b"reshade dxgi");
    }

    #[test]
    fn a_present_install_is_left_alone() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        let lines = w.log_lines();
        assert!(lines > 0);
        assert!(matches!(w.ensure(&[]), Ensured::Present(_)));
        assert!(matches!(w.ensure(&[]), Ensured::Present(_)));
        assert_eq!(w.log_lines(), lines, "install.sh did not run again");
    }

    #[test]
    fn nothing_enabled_is_not_enabled() {
        let w = World::new();
        assert!(matches!(w.ensure(&[]), Ensured::NotEnabled));
        assert_eq!(preview(&w.ctx, "local-game", &w.game, "Game.exe"), None);
    }

    #[test]
    fn a_launch_never_installs_past_a_new_kernel_anti_cheat() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        run_remove(&w.package_dir(), &w.game, &w.tmp.path().join("log")).unwrap();
        std::fs::remove_dir_all(w.game.join(BACKUP_DIR)).unwrap();
        let lines = w.log_lines();
        assert!(matches!(
            w.ensure(&[det("Ricochet", true, false)]),
            Ensured::Skipped(_)
        ));
        assert!(!w.game.join(INSTALL_MANIFEST).exists());
        assert_eq!(w.log_lines(), lines);
    }

    #[test]
    fn a_new_anti_cheat_takes_an_intact_install_out() {
        for found in [det("Ricochet", true, false), det("BattlEye", true, true)] {
            let w = World::new();
            write(&w.game.join("dxgi.dll"), THEIRS);
            w.enable(Route::ReshadeFeeder);
            assert!(preview(&w.ctx, "local-game", &w.game, "Game.exe").is_some());
            match w.ensure(std::slice::from_ref(&found)) {
                Ensured::TakenOut(reason) => assert!(reason.contains(found.name), "{reason}"),
                other => panic!("{other:?}"),
            }
            assert!(!w.game.join(INSTALL_MANIFEST).exists());
            assert_eq!(read(&w.game.join("dxgi.dll")), THEIRS);
            assert_eq!(w.entry(), None);
        }
    }

    #[test]
    fn an_anti_cheat_that_cannot_be_taken_out_blocks_the_launch() {
        let w = World::new();
        w.enable(Route::ReshadeFeeder);
        std::fs::remove_dir_all(w.package_dir()).unwrap();
        match w.ensure(&[det("Ricochet", true, false)]) {
            Ensured::Blocked(reason) => assert!(reason.contains("Ricochet"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(w.entry().is_some());
    }

    #[test]
    fn the_persons_overrides_win_and_are_reported() {
        assert_eq!(
            merge_dll_overrides(Some("dxgi=b;d3d11=n"), dll_overrides(Route::ReshadeFeeder)),
            ("dxgi=b;d3d11=n".to_string(), vec!["dxgi".to_string()])
        );
        assert_eq!(
            merge_dll_overrides(None, dll_overrides(Route::ReshadeFeederVulkan)),
            ("winevulkan=n,b;vulkan-1=n,b".to_string(), vec![])
        );
        assert_eq!(
            merge_dll_overrides(Some("d3d11,DXGI.dll=n"), &[("dxgi", "n,b")]).1,
            vec!["dxgi".to_string()]
        );
        assert_eq!(
            merge_dll_overrides(Some("d3d11=n"), &[("dxgi", "n,b")]).0,
            "d3d11=n;dxgi=n,b"
        );

        let entry = Entry {
            enabled: true,
            route: Route::ReshadeFeeder,
            model: Network::SmallFp8,
            package: "0.0.9".into(),
            arch: Arch::X86_64,
            install_dir: PathBuf::from("/g"),
        };
        let env = launch_env(&entry, Path::new("/g"), &|k| {
            (k == "WINEDLLOVERRIDES").then(|| "dxgi=b".to_string())
        });
        assert_eq!(
            env.set,
            vec![("WINEDLLOVERRIDES".to_string(), "dxgi=b".to_string())]
        );
        assert_eq!(
            env.notes,
            vec![
                "-- neural rendering: your WINEDLLOVERRIDES sets dxgi itself, so DLSSNR-AMD may not load"
                    .to_string()
            ]
        );
    }

    #[test]
    fn the_int4_layer_is_added_only_for_reshade_routes_starting_optimized() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path()
                .join("dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin"),
            b"int4",
        );
        let mut entry = Entry {
            enabled: true,
            route: Route::Optiscaler,
            model: Network::SilverAardwoldOptimized,
            package: "0.0.9".into(),
            arch: Arch::X86_64,
            install_dir: dir.path().to_path_buf(),
        };
        let none = |_: &str| None;
        let keys = |env: &LaunchEnv| env.set.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&launch_env(&entry, dir.path(), &none)),
            vec!["WINEDLLOVERRIDES"]
        );

        entry.route = Route::ReshadeFeeder;
        let current = |k: &str| (k == "VK_ADD_LAYER_PATH").then(|| "/x".to_string());
        let env = launch_env(&entry, dir.path(), &current);
        let get = |k: &str| {
            env.set
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        let layer = dir
            .path()
            .join("dlssnr-amd/int4/layer")
            .display()
            .to_string();
        assert_eq!(get("VK_ADD_LAYER_PATH"), Some(format!("/x:{layer}")));
        assert!(get("VK_ADD_LAYER_PATH")
            .unwrap()
            .ends_with("dlssnr-amd/int4/layer"));
        assert_eq!(
            get("VK_INSTANCE_LAYERS"),
            Some("VK_LAYER_DLSSNR_int4".into())
        );

        // Already there: not added twice.
        let again = |k: &str| match k {
            "VK_ADD_LAYER_PATH" => Some(layer.clone()),
            "VK_INSTANCE_LAYERS" => Some("A:VK_LAYER_DLSSNR_int4".to_string()),
            _ => None,
        };
        let env = launch_env(&entry, dir.path(), &again);
        let get = |k: &str| {
            env.set
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("VK_ADD_LAYER_PATH"), Some(layer.clone()));
        assert_eq!(
            get("VK_INSTANCE_LAYERS"),
            Some("A:VK_LAYER_DLSSNR_int4".into())
        );

        // Without the int4 file, or with another network: nothing.
        entry.model = Network::SilverAardwold;
        assert_eq!(
            keys(&launch_env(&entry, dir.path(), &none)),
            vec!["WINEDLLOVERRIDES"]
        );
        entry.model = Network::SilverAardwoldOptimized;
        std::fs::remove_file(
            dir.path()
                .join("dlssnr-amd/CC_SILVER_AARDWOLD_OPTIMIZED_INT4.bin"),
        )
        .unwrap();
        assert_eq!(
            keys(&launch_env(&entry, dir.path(), &none)),
            vec!["WINEDLLOVERRIDES"]
        );
    }

    #[test]
    fn an_enabled_game_with_the_optimized_network_installs_its_int4_file() {
        let w = World::new();
        let req = EnableRequest {
            model: Some(Network::SilverAardwoldOptimized),
            ..w.req(Some(Route::ReshadeFeeder), false)
        };
        let enabled = enable_with(&w.ctx, &req, &[]).unwrap();
        let env = launch_env(&enabled.entry, &w.game, &|_| None);
        assert!(env.set.iter().any(|(k, _)| k == "VK_INSTANCE_LAYERS"));
    }

    #[test]
    fn the_overview_reads_without_writing() {
        let w = World::new();
        let home = w.tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let paths = crate::paths::Paths::resolve(&crate::paths::Env::from_map([(
            "HOME",
            home.display().to_string(),
        )]))
        .unwrap();
        let o = overview(&w.ctx, &paths, "local-game", &w.game, "Game.exe");
        assert_eq!(o.entry, None);
        assert_eq!(o.detected.route, Some(Route::ReshadeFeeder));
        assert_eq!(o.gate, Gate::Allowed);
        assert_eq!(o.installed, None);
        assert_eq!(o.models.len(), 2);
        assert!(!o.package_ready);
        assert!(!o.dll_url_configured);
        assert!(!w.ctx.root.join("packages").exists());

        w.enable(Route::ReshadeFeeder);
        let o = overview(&w.ctx, &paths, "local-game", &w.game, "Game.exe");
        assert!(o.entry.is_some());
        assert_eq!(o.installed, Some(Ok(())));
        assert!(o.package_ready);
    }
}
