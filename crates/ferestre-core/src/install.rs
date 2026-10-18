//! What is installed, and which build of it.
//!
//! A decrypted title on disk does not reliably say what version it is: the
//! package version is a property of the MSIXVC, not of the files that come out
//! of it, and the anonymous catalog reports `Version` as `"0"` for everything.
//! So the launcher records what it installed, at the moment it installs it.
//!
//! The recorded thing was the set of **content ids**, on the belief that a
//! content id changes when a package is rebuilt. **It does not.** Measured
//! against 26 published patch plans spanning 26 distinct builds of three
//! titles: three content ids, one per title, identical across every build. A
//! content id names the title's package; the *version* names the build, and the
//! delivery path is `/{ContentId}/{VersionId}/`.
//!
//! So the update key is the package version, and the content ids are kept only
//! because they are the path component a download needs. What the anonymous
//! catalog cannot supply is the *available* version -- it reports `"0"` for
//! everything -- so the launcher asks `GetBasePackage` on the authenticated
//! update service for the available version. See docs/ROADMAP.md for delivery.
//!
//! The install does not have to be taken on trust, either. The client leaves the
//! package header on disk as `.xodus-streaming.msixvc`, and the GUID in it *is*
//! the catalog's content id -- so a title installed before this launcher existed
//! can be adopted exactly, from a 4 KiB read, with nothing to assume and nothing
//! to ask the user.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
    /// The package's content ids. **Not** the update key -- they are identical
    /// across every build of a title -- but they are the path component a
    /// download is fetched by, so they are worth having written down.
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
    /// Whether this record no longer describes something installed.
    ///
    /// Two ways that happens, and both leave a row claiming a title is there
    /// when it is not. People move and delete game directories without telling
    /// a launcher. And an install can fail while the client exits zero -- a
    /// title the account turned out not to be licensed for leaves a directory
    /// with a partial container in it and nothing else -- so the directory
    /// existing is not enough to go on.
    pub fn is_stale(&self) -> bool {
        !self.dir.is_dir() || !looks_installed(&self.dir)
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

    /// Whether a newer build than this one is available.
    ///
    /// `available` is the version the service is offering. `None` -- meaning
    /// "cannot tell" -- whenever either side is unknown, which today is always,
    /// because the anonymous catalog reports every version as `"0"` and nothing
    /// yet asks the authenticated endpoint that knows.
    ///
    /// That is deliberate and it is the whole point of this signature. The
    /// previous version compared content ids, which are identical across every
    /// build of a title, so it could only ever answer "up to date" -- a
    /// confident wrong answer to the one question this module exists to ask.
    /// "Cannot tell" is worth more than that.
    pub fn update_available(&self, available: Option<&str>) -> Option<bool> {
        let installed = self.package_version.as_deref()?;
        let available = available?;
        if installed.is_empty() || available.is_empty() {
            return None;
        }
        Some(installed != available)
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

/// Write a record. Through a temporary file and a rename, as the caches are:
/// truncating in place and then being killed leaves an empty file, which
/// [`load`] reads as "never installed" -- the title drops out of Installed and
/// an account-sharing launch loses the store content it needs.
pub fn save(state_dir: &Path, record: &Record) -> Result<()> {
    let path = record_path(state_dir, &record.product_id)
        .ok_or_else(|| anyhow!("not a usable product id: {:?}", record.product_id))?;
    crate::cache::write(&path, &serde_json::to_vec_pretty(record)?)
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

/// Whether a directory is safe for the launcher to delete.
///
/// Deleting a tree is the one thing here that cannot be undone, so the path is
/// checked before it is handed to `remove_dir_all` rather than trusted because
/// a record named it. A record is a JSON file anybody can edit, and a bug that
/// writes the wrong path into one turns into a bug that deletes a home
/// directory.
///
/// Refused: anything that is not a directory, a symlink (deleting through one
/// is how a link in a game directory takes the target with it), a path with a
/// `..` in it, the home directory, and anything within two levels of the
/// filesystem root -- `/usr`, `/home/someone` and `/` are all things a
/// launcher has no business removing whatever a record says. The comparisons
/// are made on the resolved path, so `~/xbox-games/..` is the home directory
/// it is, and so is `/home/u` reached as `/var/home/u`.
///
/// When the launcher's own directories can be resolved, everything
/// [`check_removable`] refuses is refused here too, so a caller that has not
/// been told about that check is still covered by it.
pub fn safe_to_remove(dir: &Path) -> Result<()> {
    let resolved = check_shape(dir)?;
    if let Some(home) = std::env::var_os("HOME") {
        let home = Path::new(&home);
        let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
        if resolved == home {
            anyhow::bail!("{} is the home directory", dir.display());
        }
    }
    if let Ok(paths) = crate::paths::Paths::from_env() {
        check_removable(dir, &paths)?;
    }
    Ok(())
}

/// The structural half of [`safe_to_remove`]: a real, absolute, deep enough
/// directory with no `..`. Returns the resolved path.
fn check_shape(dir: &Path) -> Result<PathBuf> {
    use anyhow::bail;
    use std::path::Component;

    if !dir.is_absolute() {
        bail!("{} is not an absolute path", dir.display());
    }
    if dir.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!(
            "{} contains \"..\"; name the directory itself",
            dir.display()
        );
    }
    if dir.is_symlink() {
        bail!("{} is a symlink; remove it by hand", dir.display());
    }
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let resolved = dir
        .canonicalize()
        .map_err(|e| anyhow!("{}: {e}", dir.display()))?;
    // RootDir plus two names: `/a/b` has three components, and is the shallowest
    // thing worth allowing.
    if resolved.components().count() < 3 || dir.components().count() < 3 {
        bail!("{} is too close to the root to remove", dir.display());
    }
    Ok(resolved)
}

/// Whether `dir` may be deleted as one title's folder (or one title's
/// prefix).
///
/// Everything [`safe_to_remove`] checks for shape, and then what only the
/// launcher's directories can say. Refused:
///
///   * the home directory, the games directory, the launcher's state, config
///     and cache directories, the XDG base directories, the runtime, client
///     and build trees, Steam -- and any directory that contains one of them;
///   * a directory holding another title's recorded install;
///   * a directory with another download or another title's prefix directly
///     inside it. That is what a library folder chosen as an install
///     destination looks like, and removing "the title" would remove the
///     library.
pub fn check_removable(dir: &Path, paths: &crate::paths::Paths) -> Result<()> {
    use anyhow::bail;

    let resolved = check_shape(dir)?;
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());

    for (what, protected) in protected_dirs(paths) {
        let protected = resolve(&protected);
        if protected == resolved {
            bail!("{} is {what}; it is never removed", dir.display());
        }
        if protected.starts_with(&resolved) {
            bail!(
                "{} contains {what} ({}); it is never removed",
                dir.display(),
                protected.display()
            );
        }
    }

    let mut owners = Vec::new();
    for record in all(paths.state_dir()) {
        if !record.dir.is_absolute() {
            continue;
        }
        let recorded = resolve(&record.dir);
        if recorded == resolved {
            owners.push(record.product_id);
        } else if recorded.starts_with(&resolved) {
            bail!(
                "{} holds the install of {} ({}); remove that first",
                dir.display(),
                record.product_id,
                record.dir.display()
            );
        }
    }
    if owners.len() > 1 {
        bail!(
            "{} is recorded as the folder of {}; it is not one title's folder",
            dir.display(),
            owners.join(" and ")
        );
    }

    if let Ok(entries) = std::fs::read_dir(&resolved) {
        for entry in entries.flatten() {
            let child = entry.path();
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if child.join(CONTAINER).is_file() || child.join(PARTIAL_CONTAINER).is_file() {
                bail!(
                    "{} has another download inside it ({}); it looks like a library \
                     folder, not one title's folder",
                    dir.display(),
                    child.display()
                );
            }
            if name.ends_with("-proton") && child.join("pfx").is_dir() {
                bail!(
                    "{} has a title's Wine prefix inside it ({}); it looks like a games \
                     folder, not one title's folder",
                    dir.display(),
                    child.display()
                );
            }
        }
    }
    Ok(())
}

/// The directories nothing may remove, or remove the parent of, with what to
/// call each in a refusal.
fn protected_dirs(paths: &crate::paths::Paths) -> Vec<(&'static str, PathBuf)> {
    let home = paths.home().to_path_buf();
    let mut dirs = vec![
        ("the home directory", home.clone()),
        ("the games directory", paths.games_dir().to_path_buf()),
        (
            "the launcher's state directory",
            paths.state_dir().to_path_buf(),
        ),
        (
            "the launcher's config directory",
            paths.config_dir().to_path_buf(),
        ),
        (
            "the launcher's cache directory",
            paths.cache_dir().to_path_buf(),
        ),
        ("the runtimes directory", paths.runtimes_dir()),
        ("the build directory", paths.build_dir().to_path_buf()),
        ("a standard XDG directory", home.join(".local/share")),
        ("a standard XDG directory", home.join(".local/state")),
        ("a standard XDG directory", home.join(".config")),
        ("a standard XDG directory", home.join(".cache")),
    ];
    for (what, dir) in [
        ("the launcher's installed tree", paths.tree_dir()),
        ("the client directory", paths.cli_dir()),
        ("the runtime", paths.runtime_dir()),
        ("Steam", paths.steam_dir()),
    ] {
        if let Some(dir) = dir {
            dirs.push((what, dir.to_path_buf()));
        }
    }
    for name in [
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        if let Some(value) = paths.var(name).filter(|v| Path::new(v).is_absolute()) {
            dirs.push(("a standard XDG directory", PathBuf::from(value)));
        }
    }
    dirs
}

/// Where to install a title, given what someone typed into "Install to".
///
/// The typed folder is a library: the title goes into `folder` inside it, so
/// choosing `/mnt/games` never makes `/mnt/games` the title's own folder, and
/// removing the title later can only ever remove `/mnt/games/<folder>`.
/// `folder` is the title's usual directory name, such as a recipe's
/// [`crate::recipe::Recipe::install_dir`] or the lowercase product id.
///
/// The one exception is a folder that already is this title's install --
/// its recorded directory, or a folder whose package names this product --
/// which is used as it is, so a repair or an update lands on the existing
/// files rather than in a new copy one level down. A download that was
/// cancelled before it named its product counts as this title's too, chosen
/// directly or found at `<chosen>/<folder>`: a first download has no record
/// yet, and Resume has to continue it rather than be refused.
///
/// `<chosen>/<folder>` may already exist only if it is empty, this title's,
/// or such a download. Anything else in it is somebody's files, and they
/// would go with the title when it is removed.
///
/// A leading `~` means the home directory; anything else must be absolute.
pub fn resolve_destination(
    typed: &str,
    folder: &str,
    product_id: &str,
    paths: &crate::paths::Paths,
) -> Result<PathBuf> {
    use anyhow::bail;
    use std::path::Component;

    let typed = typed.trim();
    let chosen = if typed == "~" {
        paths.home().to_path_buf()
    } else if let Some(rest) = typed.strip_prefix("~/") {
        paths.home().join(rest)
    } else {
        PathBuf::from(typed)
    };
    if typed.is_empty() || !chosen.is_absolute() {
        bail!(
            "\"{typed}\" is not a folder to install into; give a full path such as {}",
            paths.games_dir().display()
        );
    }
    if chosen
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        bail!("\"{typed}\" contains \"..\"; give the folder itself");
    }
    let folder_path = Path::new(folder);
    if folder.is_empty()
        || !folder_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("\"{folder}\" is not a folder name");
    }

    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let owner = |dir: &Path| owner_of(dir, product_id, paths);

    let destination = match owner(&chosen) {
        Owner::This | Owner::Unknown => chosen,
        Owner::Other(other) => bail!(
            "{} is the folder of {other}; choose the folder your games go in",
            chosen.display()
        ),
        Owner::Nobody => chosen.join(folder_path),
    };

    match owner(&destination) {
        Owner::This | Owner::Unknown => {}
        Owner::Other(other) => bail!(
            "{} already holds the files of {other}; choose another folder",
            destination.display()
        ),
        Owner::Nobody => {
            if destination.exists() && !destination.is_dir() {
                bail!(
                    "{} is a file, not a folder; choose another folder",
                    destination.display()
                );
            }
            let occupied = std::fs::read_dir(&destination)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
            if occupied {
                bail!(
                    "{} already exists and holds other files; the title needs a folder \
                     of its own, and removing it later would remove those too. Move them \
                     or choose another folder",
                    destination.display()
                );
            }
        }
    }
    let resolved = resolve(&destination);
    for (what, protected) in protected_dirs(paths) {
        let protected = resolve(&protected);
        if protected == resolved || protected.starts_with(&resolved) {
            bail!(
                "{} is {what}, or contains it; a title cannot be installed there",
                destination.display()
            );
        }
    }
    Ok(destination)
}

/// Whose files a folder holds, as far as [`resolve_destination`] can tell.
enum Owner {
    /// This product's: its recorded install, or a package naming it.
    This,
    /// Another product's, by its record or its package.
    Other(String),
    /// A download that started and has not got as far as naming its
    /// product. A first download leaves no record until it succeeds and the
    /// package's config is often not there yet, so this is what a cancelled
    /// first download looks like -- and it has to be resumable.
    Unknown,
    /// No package at all: nothing, or somebody's other files.
    Nobody,
}

fn owner_of(dir: &Path, product_id: &str, paths: &crate::paths::Paths) -> Owner {
    if !dir.is_dir() {
        return Owner::Nobody;
    }
    let resolve = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let resolved = resolve(dir);
    let mut recorded_here = all(paths.state_dir())
        .into_iter()
        .filter(|record| record.dir.is_absolute() && resolve(&record.dir) == resolved)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    if recorded_here
        .iter()
        .any(|id| id.eq_ignore_ascii_case(product_id))
    {
        return if recorded_here.len() == 1 {
            Owner::This
        } else {
            recorded_here.retain(|id| !id.eq_ignore_ascii_case(product_id));
            Owner::Other(recorded_here.join(" and "))
        };
    }
    if !recorded_here.is_empty() {
        return Owner::Other(recorded_here.join(" and "));
    }
    let installed = looks_installed(dir);
    let partial = is_partial(dir);
    if !installed && !partial {
        return Owner::Nobody;
    }
    match store_id(dir) {
        Some(id) if id.eq_ignore_ascii_case(product_id) => Owner::This,
        Some(id) => Owner::Other(id),
        None if partial => Owner::Unknown,
        None => Owner::Other("a title the launcher cannot name".into()),
    }
}

/// Bytes used by a directory tree, following no symlinks.
///
/// Best effort: a file that cannot be read contributes nothing rather than
/// failing the walk, because this exists to put a number in a sentence, not to
/// audit a filesystem.
pub fn tree_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => tree_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Allocated bytes for game files, without following symlinks or counting a
/// hardlinked inode twice. Unlike file lengths, this handles sparse packages.
/// An unreadable tree has no trustworthy total.
pub fn disk_usage(dir: &Path) -> Option<u64> {
    use std::collections::BTreeSet;
    use std::os::unix::fs::MetadataExt;

    fn walk(path: &Path, seen: &mut BTreeSet<(u64, u64)>) -> Option<u64> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink() || !seen.insert((metadata.dev(), metadata.ino())) {
            return Some(0);
        }
        let mut bytes = metadata.blocks().saturating_mul(512);
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).ok()? {
                bytes = bytes.saturating_add(walk(&entry.ok()?.path(), seen)?);
            }
        }
        Some(bytes)
    }

    walk(&dir.canonicalize().ok()?, &mut BTreeSet::new())
}

/// A cheap identity check, not a recursive walk. The daily expiry catches
/// changes to files deep inside a game that do not alter its root directory.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct UsageFingerprint {
    dir: PathBuf,
    root: UsageStamp,
    manifest: Option<UsageStamp>,
    record: Option<Record>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct UsageStamp {
    device: u64,
    inode: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    len: u64,
}

impl UsageStamp {
    fn read(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            len: metadata.len(),
        })
    }
}

impl UsageFingerprint {
    fn read(dir: &Path, record: Option<&Record>) -> Option<Self> {
        let dir = dir.canonicalize().ok()?;
        let root = UsageStamp::read(&dir)?;
        let manifest = ["appxmanifest.xml", "AppxManifest.xml"]
            .iter()
            .find_map(|name| UsageStamp::read(&dir.join(name)));
        Some(Self {
            dir,
            root,
            manifest,
            record: record.cloned(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct UsageEntry {
    schema: u32,
    fingerprint: UsageFingerprint,
    bytes: u64,
}

pub fn disk_usage_cache_path(cache_dir: &Path, product_id: &str) -> Option<PathBuf> {
    let name = record_path(cache_dir, product_id)?.file_name()?.to_owned();
    Some(cache_dir.join("disk-usage").join(name))
}

/// Last good allocated-byte measurement and whether it is still fresh.
/// Reading this only stats the root/manifest and reads one small cache file.
pub fn cached_disk_usage(
    cache_dir: &Path,
    product_id: &str,
    dir: &Path,
    record: Option<&Record>,
) -> Option<(u64, bool)> {
    let path = disk_usage_cache_path(cache_dir, product_id)?;
    let entry: UsageEntry = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if entry.schema != 1 || entry.fingerprint != UsageFingerprint::read(dir, record)? {
        return None;
    }
    Some((
        entry.bytes,
        crate::cache::is_fresh(&path, crate::cache::DISK_USAGE_MAX_AGE),
    ))
}

/// Measure off the UI thread and atomically keep the result for future loads.
/// Do not cache an unreadable or detectably changing install.
pub fn measure_disk_usage(
    cache_dir: &Path,
    product_id: &str,
    dir: &Path,
    record: Option<&Record>,
) -> Option<u64> {
    let fingerprint = UsageFingerprint::read(dir, record)?;
    let bytes = disk_usage(dir)?;
    if UsageFingerprint::read(dir, record).as_ref() == Some(&fingerprint) {
        if let Some(path) = disk_usage_cache_path(cache_dir, product_id) {
            let entry = UsageEntry {
                schema: 1,
                fingerprint,
                bytes,
            };
            if let Ok(json) = serde_json::to_vec(&entry) {
                let _ = crate::cache::write(&path, &json);
            }
        }
    }
    Some(bytes)
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

/// What the client downloads into, renamed to [`CONTAINER`] only once the
/// whole download has finished. While it is there, the download is not.
pub const PARTIAL_CONTAINER: &str = ".xodus-streaming-tmp.msixvc";

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

/// The executable a package declares as its entry point.
///
/// From the manifest, not from guessing: `<Application Executable="...">` is
/// what Windows itself launches, so it is right by construction. That is the
/// difference between a launcher that asks someone to find an .exe in a tree of
/// thousands and one that does not ask.
///
/// A starting point, not always the final answer. Some titles name a launcher
/// shim here and the real binary is deeper -- Clair Obscur declares
/// `SandFall.exe` while the recipe that works points at
/// `Sandfall\Binaries\WinGDK\SandFall-WinGDK-Shipping.exe`. So this fills the
/// field in and leaves it editable, rather than deciding for the person.
pub fn executable(install_dir: &Path) -> Option<String> {
    let manifest = ["appxmanifest.xml", "AppxManifest.xml"]
        .iter()
        .map(|name| install_dir.join(name))
        .find(|path| path.is_file())?;
    let text = std::fs::read_to_string(manifest).ok()?;

    // The first <Application>: a package may declare several, and the first is
    // the one the Store shows.
    let start = text.find("<Application ")?;
    let end = text[start..].find('>')? + start;
    attribute(&text[start..end], "Executable")
}

/// One attribute out of one element, without a parser. Attribute order is not
/// guaranteed, and assuming it is exactly the kind of thing that works on the
/// two titles you happened to test with.
fn attribute(element: &str, name: &str) -> Option<String> {
    let at = element.find(&format!("{name}="))?;
    let rest = &element[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let close = value.find(quote)?;
    let value = value[..close].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Whether this directory holds an installed title, rather than the debris of
/// an install that did not happen.
///
/// The check exists because the client can fail and exit zero. A Game Pass
/// title the account turned out not to be entitled to printed
///
/// ```text
/// not entitled to this content: The entitlement has been revoked and can no
/// longer be used to create a Content License.
/// ```
///
/// and then exited successfully, leaving a directory with nothing in it but a
/// 4 MB `.xodus-streaming-tmp.msixvc`. The launcher believed it, wrote a record,
/// and the title sat in the library claiming to be installed forever.
///
/// Two layouts count. A package whose finished header the client kept beside
/// it is an install. So is a decrypted tree with a manifest, as long as no
/// download is still under way in it: the client writes the package's files in
/// whatever order they arrive, so the manifest can be there long before the
/// rest, and the *temporary* container is exactly what a half-finished
/// download leaves. [`is_partial`] says which titles are in that state.
pub fn looks_installed(dir: &Path) -> bool {
    dir.join(CONTAINER).is_file()
        || (["appxmanifest.xml", "AppxManifest.xml"]
            .iter()
            .any(|name| dir.join(name).is_file())
            && !is_partial(dir))
}

/// Whether a download into this directory started and has not finished: the
/// state to offer Resume or Remove for, rather than Play.
pub fn is_partial(dir: &Path) -> bool {
    dir.join(PARTIAL_CONTAINER).exists()
}

/// Product ids named by installed packages below a games directory.
///
/// Install directories are deliberately not used as identities. Older client
/// versions, manually moved games, and recipes all choose useful directory
/// names such as `fh5`; the Store catalogue names the same package by its
/// `<StoreId>` in `MicrosoftGame.Config`. Reading that identity is what lets a
/// Game Pass row recognise an existing install before offering another one.
pub fn installed_products(games_dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(games_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir() && looks_installed(dir))
        .filter_map(|dir| store_id(&dir))
        .collect()
}

/// The package's `MicrosoftGame.config`, whatever its case.
///
/// Microsoft has shipped both `.config` and `.Config`, and the package runs on
/// Windows where every spelling is the same file, so recognising only some
/// creates false answers. The two known spellings are tried first because they
/// cost a stat each; anything else costs one directory listing.
pub fn game_config(install_dir: &Path) -> Option<PathBuf> {
    const NAME: &str = "MicrosoftGame.config";
    ["MicrosoftGame.config", "MicrosoftGame.Config"]
        .iter()
        .map(|name| install_dir.join(name))
        .find(|path| path.is_file())
        .or_else(|| {
            std::fs::read_dir(install_dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case(NAME))
                        && path.is_file()
                })
        })
}

/// The package identity name in MicrosoftGame.config, independent of attribute order.
pub fn package_identity(install_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(game_config(install_dir)?).ok()?;
    let start = text.find("<Identity")?;
    let element = text[start..].split_once('>')?.0;
    let rest = element
        .split_whitespace()
        .find_map(|attribute| attribute.strip_prefix("Name="))?;
    let quote = rest.chars().next()?;
    if quote != '\"' && quote != '\'' {
        return None;
    }
    let name = rest[1..].split_once(quote)?.0;
    (!name.is_empty()).then(|| name.to_string())
}

fn store_id(install_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(game_config(install_dir)?).ok()?;
    let open = "<StoreId>";
    let start = text.find(open)? + open.len();
    let value = text[start..].split_once("</StoreId>")?.0.trim();
    (!value.is_empty()).then(|| value.to_ascii_uppercase())
}

/// The identity a title signs in to Xbox under, as the runtime reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleIdentity {
    /// `TitleId` in decimal, the way the runtime sends it; empty when the
    /// config names no title.
    pub title_id: String,
    /// `MSAAppId`; empty when the config has none, or names no title.
    pub client_id: String,
}

/// The title identity in an install's `MicrosoftGame.config`, read the way
/// the runtime reads it: the text of the first `<TitleId>` element as
/// hexadecimal, and of the first `<MSAAppId>`, each under 64 bytes.
///
/// The runtime sends this pair on every token request, and a title is free to
/// put anything there instead. Read here, before the title runs, it is what the
/// launcher binds the service to, so the service can tell the title's own
/// identity from one it claims. `None` when the install has no readable
/// config.
pub fn title_identity(install_dir: &Path) -> Option<TitleIdentity> {
    let text = std::fs::read_to_string(game_config(install_dir)?).ok()?;
    let element = |name: &str| {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let value = &text[start..];
        let value = &value[..value.find('<').unwrap_or(value.len())];
        (value.len() < 64).then_some(value)
    };
    // strtoul: leading space, an optional 0x, then as many hex digits as
    // there are. A value that is zero or does not fit a title ID is no title.
    let title = element("TitleId").and_then(|hex| {
        let hex = hex.trim_start();
        let hex = hex
            .strip_prefix("0x")
            .or_else(|| hex.strip_prefix("0X"))
            .unwrap_or(hex);
        let digits = hex.len()
            - hex
                .trim_start_matches(|c: char| c.is_ascii_hexdigit())
                .len();
        u64::from_str_radix(&hex[..digits], 16)
            .ok()
            .filter(|id| *id > 0 && *id <= u64::from(u32::MAX))
    });
    Some(match title {
        Some(id) => TitleIdentity {
            title_id: id.to_string(),
            client_id: element("MSAAppId").unwrap_or("").trim().to_string(),
        },
        None => TitleIdentity::default(),
    })
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

    /// The guard on the one irreversible thing here. A record is a JSON file
    /// anybody can edit, so the path in it is checked rather than trusted.
    #[test]
    fn disk_usage_cache_survives_loads_and_invalidates_changed_installs() {
        use std::time::{Duration, SystemTime};
        let base =
            std::env::temp_dir().join(format!("ferestre-usage-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let game = base.join("game");
        let cache = base.join("cache");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("data/content"), b"old").unwrap();
        std::fs::write(game.join("appxmanifest.xml"), b"version one").unwrap();
        let record = Record {
            product_id: "9ZZTESTGAME1".into(),
            dir: game.clone(),
            content_ids: vec![],
            installed_at: Some("2026-10-17T00:00:00Z".into()),
            package_version: Some("1".into()),
        };
        assert!(cached_disk_usage(&cache, &record.product_id, &game, Some(&record)).is_none());
        let measured =
            measure_disk_usage(&cache, &record.product_id, &game, Some(&record)).unwrap();
        assert_eq!(measured, disk_usage(&game).unwrap());
        let path = disk_usage_cache_path(&cache, &record.product_id).unwrap();
        let snapshot = std::fs::read(&path).unwrap();
        // Existing deep content changes do not require an expensive recursive
        // fingerprint. A fresh cache reuses its measurement; expiry updates it.
        std::fs::write(game.join("data/content"), [7; 128 * 1024]).unwrap();
        assert_eq!(
            cached_disk_usage(&cache, &record.product_id, &game, Some(&record)),
            Some((measured, true))
        );
        assert_eq!(std::fs::read(&path).unwrap(), snapshot);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(48 * 3600)),
            )
            .unwrap();
        assert_eq!(
            cached_disk_usage(&cache, &record.product_id, &game, Some(&record)),
            Some((measured, false))
        );
        let updated = measure_disk_usage(&cache, &record.product_id, &game, Some(&record)).unwrap();
        assert!(updated > measured);
        assert_eq!(
            cached_disk_usage(&cache, &record.product_id, &game, Some(&record)),
            Some((updated, true))
        );
        let mut new_record = record.clone();
        new_record.package_version = Some("2".into());
        assert!(cached_disk_usage(&cache, &record.product_id, &game, Some(&new_record)).is_none());
        std::fs::write(game.join("appxmanifest.xml"), b"version two changed").unwrap();
        assert!(cached_disk_usage(&cache, &record.product_id, &game, Some(&record)).is_none());
        measure_disk_usage(&cache, &record.product_id, &game, Some(&record)).unwrap();
        std::fs::write(game.join("new-package"), b"more content").unwrap();
        assert!(cached_disk_usage(&cache, &record.product_id, &game, Some(&record)).is_none());
        let moved = base.join("moved");
        std::fs::rename(&game, &moved).unwrap();
        assert!(cached_disk_usage(&cache, &record.product_id, &moved, Some(&record)).is_none());
        let before_failure = std::fs::read(&path).unwrap();
        assert!(measure_disk_usage(&cache, &record.product_id, &game, Some(&record)).is_none());
        assert_eq!(std::fs::read(&path).unwrap(), before_failure);
        assert!(disk_usage_cache_path(&cache, "../../escape").is_none());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn disk_usage_counts_allocations_once_and_does_not_follow_links() {
        use std::os::unix::fs::{symlink, MetadataExt};
        let dir = std::env::temp_dir().join(format!("ferestre-allocated-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("sparse")).unwrap();
        file.set_len(1_000_000_000).unwrap();
        std::fs::hard_link(dir.join("sparse"), dir.join("hardlink")).unwrap();
        symlink("/", dir.join("outside")).unwrap();
        let expected = std::fs::metadata(&dir).unwrap().blocks() * 512
            + file.metadata().unwrap().blocks() * 512;
        assert_eq!(disk_usage(&dir), Some(expected));
        assert!(disk_usage(&dir).unwrap() < 1_000_000_000);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(disk_usage(&dir), None);
    }

    #[test]
    fn shallow_paths_and_symlinks_are_refused() {
        assert!(safe_to_remove(Path::new("/")).is_err());
        assert!(safe_to_remove(Path::new("/usr")).is_err(), "one level down");
        assert!(safe_to_remove(Path::new("relative/path")).is_err());
        // Deep enough as text, and the root once resolved.
        assert!(safe_to_remove(Path::new("/usr/lib/../..")).is_err());
        if let Some(home) = std::env::var_os("HOME") {
            let home = Path::new(&home);
            assert!(safe_to_remove(&home.join("xbox-games/..")).is_err());
            assert!(safe_to_remove(&home.join("x/../..")).is_err());
        }
        assert!(
            safe_to_remove(Path::new("/nonexistent/deep/enough")).is_err(),
            "a path that is not there is not a directory to remove"
        );

        let dir = std::env::temp_dir().join(format!("ferestre-rm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("game")).expect("temp dir");
        assert!(safe_to_remove(&dir.join("game")).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A scratch home with a games directory, resolved the way the launcher
    /// resolves it.
    struct Home {
        root: PathBuf,
        paths: crate::paths::Paths,
    }

    impl Home {
        fn new(tag: &str) -> Home {
            let root = std::env::temp_dir()
                .join(format!("ferestre-home-{tag}-{}", std::process::id()))
                .join("home/tester");
            let _ = std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap());
            std::fs::create_dir_all(root.join("xbox-games")).unwrap();
            let paths = crate::paths::Paths::resolve(&crate::paths::Env::from_map([(
                "HOME",
                root.to_str().unwrap(),
            )]))
            .unwrap();
            Home { root, paths }
        }

        fn games(&self) -> PathBuf {
            self.root.join("xbox-games")
        }

        fn game(&self, name: &str) -> PathBuf {
            let dir = self.games().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(CONTAINER), b"header").unwrap();
            dir
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root.parent().unwrap().parent().unwrap());
        }
    }

    /// `ferestre uninstall ..` and `uninstall ""` were both reproduced
    /// deleting a home directory and a games directory. Neither path is a
    /// title's folder, however it is spelled.
    #[test]
    fn the_home_and_games_directories_are_never_removable() {
        let home = Home::new("guard");
        let game = home.game("example");
        let p = &home.paths;

        assert!(check_removable(&game, p).is_ok());
        for refused in [
            home.games(),
            home.games().join(""),
            home.games().join(".."),
            home.games().join("example/.."),
            home.games().join("example/../.."),
            home.root.clone(),
            home.root.parent().unwrap().to_path_buf(),
        ] {
            assert!(
                check_removable(&refused, p).is_err(),
                "{} must be refused",
                refused.display()
            );
        }
        // The launcher's own directories, once they exist.
        for dir in [p.state_dir(), p.config_dir(), p.cache_dir()] {
            std::fs::create_dir_all(dir).unwrap();
            assert!(check_removable(dir, p).is_err(), "{}", dir.display());
            assert!(
                check_removable(dir.parent().unwrap(), p).is_err(),
                "{}",
                dir.display()
            );
        }
        // Reached through a symlinked parent, the home directory is still it.
        let link = home.root.parent().unwrap().join("alias");
        std::os::unix::fs::symlink(&home.root, &link).unwrap();
        assert!(check_removable(&link.join("xbox-games"), p).is_err());
        assert!(check_removable(&link.join("xbox-games/example"), p).is_ok());
    }

    /// A library folder chosen as an install destination: other games and
    /// prefixes inside it, and a record naming it.
    #[test]
    fn a_folder_holding_other_titles_is_not_one_titles_folder() {
        let home = Home::new("library");
        let library = home.root.join("library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("appxmanifest.xml"), "<Package/>").unwrap();
        let p = &home.paths;
        assert!(check_removable(&library, p).is_ok());

        std::fs::create_dir_all(library.join("other-game")).unwrap();
        std::fs::write(library.join("other-game").join(PARTIAL_CONTAINER), b"x").unwrap();
        let err = check_removable(&library, p).unwrap_err().to_string();
        assert!(err.contains("another download"), "{err}");
        std::fs::remove_dir_all(library.join("other-game")).unwrap();

        std::fs::create_dir_all(library.join("other-proton/pfx")).unwrap();
        let err = check_removable(&library, p).unwrap_err().to_string();
        assert!(err.contains("prefix"), "{err}");
        std::fs::remove_dir_all(library.join("other-proton")).unwrap();

        // Another title's recorded install lives below it.
        let nested = library.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let mut other = record(&[]);
        other.product_id = "9ZZOTHERGAME".into();
        other.dir = nested.clone();
        save(p.state_dir(), &other).unwrap();
        let err = check_removable(&library, p).unwrap_err().to_string();
        assert!(err.contains("9ZZOTHERGAME"), "{err}");
        // That title's own folder is still removable.
        assert!(check_removable(&nested, p).is_ok());
        forget(p.state_dir(), "9ZZOTHERGAME").unwrap();

        // Two titles recorded in the same folder: neither owns it.
        for id in ["9ZZTESTGAME1", "9ZZTESTGAME2"] {
            let mut shared = record(&[]);
            shared.product_id = id.into();
            shared.dir = library.clone();
            save(p.state_dir(), &shared).unwrap();
        }
        assert!(check_removable(&library, p).is_err());
    }

    #[test]
    fn install_to_names_a_library_and_the_title_gets_its_own_folder() {
        let home = Home::new("destination");
        let p = &home.paths;
        let library = home.root.join("library");
        std::fs::create_dir_all(library.join("someone-elses-stuff")).unwrap();

        assert_eq!(
            resolve_destination(library.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            library.join("fh5")
        );
        // A recipe's layout below the chosen folder is kept.
        assert_eq!(
            resolve_destination(library.to_str().unwrap(), "bedrock/game", "9NBLGGH2JHXJ", p)
                .unwrap(),
            library.join("bedrock/game")
        );
        // "~" is the home directory, not a folder called "~".
        assert_eq!(
            resolve_destination("~/library", "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            library.join("fh5")
        );
        assert_eq!(
            resolve_destination(" ~ ", "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            home.root.join("fh5")
        );
        for bad in ["", "relative/dir", "~someone/games", "/mnt/../etc"] {
            assert!(
                resolve_destination(bad, "fh5", "9NNX1VVR3KNQ", p).is_err(),
                "{bad:?}"
            );
        }
        for bad in ["", "..", "../x", "/abs"] {
            assert!(
                resolve_destination(library.to_str().unwrap(), bad, "9NNX1VVR3KNQ", p).is_err(),
                "folder {bad:?}"
            );
        }

        // Choosing the title's own folder repairs it in place: by its record,
        // or by the package's own Store id.
        let own = home.game("fh5");
        std::fs::write(
            own.join("MicrosoftGame.Config"),
            "<Game><StoreId>9NNX1VVR3KNQ</StoreId></Game>",
        )
        .unwrap();
        assert_eq!(
            resolve_destination(own.to_str().unwrap(), "fh5", "9nnx1vvr3knq", p).unwrap(),
            own
        );
        let recorded = home.root.join("elsewhere/forza");
        std::fs::create_dir_all(&recorded).unwrap();
        let mut forza = record(&[]);
        forza.product_id = "9NNX1VVR3KNQ".into();
        forza.dir = recorded.clone();
        save(p.state_dir(), &forza).unwrap();
        assert_eq!(
            resolve_destination(recorded.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            recorded
        );
        // Somebody else's game in the way is refused, not mixed into.
        assert!(resolve_destination(own.to_str().unwrap(), "fh5", "9ZZOTHERGAME", p).is_err());
        // The parent of the games directory cannot become a title's folder by
        // way of its name.
        assert!(resolve_destination(
            home.root.parent().unwrap().to_str().unwrap(),
            "tester",
            "9NNX1VVR3KNQ",
            p
        )
        .is_err());
    }

    /// A first download leaves no record until it finishes, and its config
    /// usually has not arrived when it is cancelled. Resume has to land on
    /// it again, wherever it was, not be refused as somebody else's title.
    #[test]
    fn a_cancelled_first_download_can_be_resumed_where_it_is() {
        let home = Home::new("resume");
        let p = &home.paths;
        let partial = |dir: &Path| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(PARTIAL_CONTAINER), b"part").unwrap();
        };

        let library = home.root.join("library");
        partial(&library.join("fh5"));
        let fh5 = library.join("fh5");
        assert_eq!(
            resolve_destination(library.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            fh5
        );
        assert_eq!(
            resolve_destination(fh5.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            fh5
        );
        partial(&home.games().join("fh5"));
        assert_eq!(
            resolve_destination(home.games().to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            home.games().join("fh5")
        );

        // Once the download names its product, or a record names another
        // one, it is that title's and nobody else resumes into it.
        std::fs::write(
            fh5.join("MicrosoftGame.Config"),
            "<Game><StoreId>9ZZOTHERGAME</StoreId></Game>",
        )
        .unwrap();
        for typed in [&library, &fh5] {
            let err = resolve_destination(typed.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p)
                .unwrap_err()
                .to_string();
            assert!(err.contains("9ZZOTHERGAME"), "{err}");
        }
        let games_fh5 = home.games().join("fh5");
        let mut other = record(&[]);
        other.product_id = "9ZZOTHERGAME".into();
        other.dir = games_fh5.clone();
        save(p.state_dir(), &other).unwrap();
        let err = resolve_destination(home.games().to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("9ZZOTHERGAME"), "{err}");
    }

    /// The title's own folder is what Remove deletes later, so it cannot be
    /// a folder that already holds somebody's other files.
    #[test]
    fn a_title_is_not_installed_over_somebody_elses_files() {
        let home = Home::new("occupied");
        let p = &home.paths;
        let library = home.root.join("lib2");
        std::fs::create_dir_all(library.join("fh5")).unwrap();
        // An empty folder of the right name is fine.
        assert_eq!(
            resolve_destination(library.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).unwrap(),
            library.join("fh5")
        );
        std::fs::write(library.join("fh5/my-notes.txt"), b"mine").unwrap();
        let err = resolve_destination(library.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("other files"), "{err}");
        // Choosing that folder itself puts the title one level down, which
        // is empty, so that is allowed and leaves the notes alone.
        assert_eq!(
            resolve_destination(
                library.join("fh5").to_str().unwrap(),
                "fh5",
                "9NNX1VVR3KNQ",
                p
            )
            .unwrap(),
            library.join("fh5/fh5")
        );
        // A file where the folder would go.
        std::fs::write(library.join("bedrock"), b"not a folder").unwrap();
        assert!(
            resolve_destination(library.to_str().unwrap(), "bedrock", "9NBLGGH2JHXJ", p).is_err()
        );
        // A package that names nothing and is not a download in progress is
        // still somebody's.
        let nameless = home.game("nameless");
        assert!(resolve_destination(
            home.games().to_str().unwrap(),
            "nameless",
            "9NNX1VVR3KNQ",
            p
        )
        .is_err());
        assert!(resolve_destination(nameless.to_str().unwrap(), "fh5", "9NNX1VVR3KNQ", p).is_err());
    }

    #[test]
    fn a_tree_is_measured_including_what_is_under_it() {
        let dir = std::env::temp_dir().join(format!("ferestre-size-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("data")).expect("temp dir");
        std::fs::write(dir.join("a"), vec![0u8; 1000]).expect("write");
        std::fs::write(dir.join("data/b"), vec![0u8; 2000]).expect("write");
        assert_eq!(tree_size(&dir), 3000);
        assert_eq!(tree_size(Path::new("/nonexistent")), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The state a failed install leaves, and the reason this check exists: the
    /// client printed "not entitled to this content" and exited zero, so
    /// everything downstream treated an empty directory as a finished install.
    #[test]
    fn a_directory_holding_only_a_partial_download_is_not_an_install() {
        let dir = std::env::temp_dir().join(format!("ferestre-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(
            !looks_installed(&dir),
            "an empty directory is not an install"
        );

        std::fs::write(dir.join(".xodus-streaming-tmp.msixvc"), b"partial").expect("write");
        assert!(
            !looks_installed(&dir),
            "and neither is one holding only the temporary container"
        );

        // The client writes files in whatever order they arrive, so the
        // manifest can land long before the download finishes.
        std::fs::write(dir.join("appxmanifest.xml"), b"<Package/>").expect("write");
        assert!(
            !looks_installed(&dir),
            "a manifest beside an unfinished download is not an install"
        );
        assert!(is_partial(&dir));

        std::fs::remove_file(dir.join(PARTIAL_CONTAINER)).expect("finish");
        assert!(looks_installed(&dir), "a manifest is a decrypted title");
        assert!(!is_partial(&dir));

        // An update in progress over a finished install: the old header is
        // still there, and so is most of the old build.
        std::fs::write(dir.join(CONTAINER), b"header").expect("write");
        std::fs::write(dir.join(PARTIAL_CONTAINER), b"partial").expect("write");
        assert!(looks_installed(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The other legitimate layout: the client keeps the package header beside
    /// a title it streamed, and there is no decrypted manifest at the top.
    #[test]
    fn a_directory_with_the_package_header_is_an_install() {
        let dir = std::env::temp_dir().join(format!("ferestre-streamed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(CONTAINER), b"header").expect("write");
        assert!(looks_installed(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_game_config_is_found_whatever_its_case() {
        let dir = std::env::temp_dir().join(format!("ferestre-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(game_config(&dir), None);
        std::fs::write(dir.join("MICROSOFTGAME.CONFIG"), "<Game/>").expect("config");
        assert_eq!(game_config(&dir), Some(dir.join("MICROSOFTGAME.CONFIG")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn installed_products_uses_the_package_store_id_not_the_directory_name() {
        let root = std::env::temp_dir().join(format!("ferestre-products-{}", std::process::id()));
        let game = root.join("a-legacy-directory-name");
        std::fs::create_dir_all(&game).expect("temp dir");
        std::fs::write(game.join(CONTAINER), b"header").expect("write");
        std::fs::write(
            game.join("MicrosoftGame.Config"),
            "<Game><StoreId>9ZZTESTGAME</StoreId></Game>",
        )
        .expect("config");

        assert_eq!(
            installed_products(&root).into_iter().collect::<Vec<_>>(),
            vec!["9ZZTESTGAME"]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    fn record(content_ids: &[&str]) -> Record {
        Record {
            product_id: "9ZZTESTGAME1".into(),
            dir: PathBuf::from("/games/test"),
            content_ids: content_ids.iter().map(|s| s.to_string()).collect(),
            installed_at: Some("2026-09-06T09:00:00Z".into()),
            package_version: Some("1.26.4501.0".into()),
        }
    }

    /// The correction this signature exists for. Content ids do not change
    /// between builds -- 26 published plans across 26 builds of three titles
    /// carry three content ids, one per title -- so comparing them answered
    /// "up to date" to every real update. A version comparison answers it.
    #[test]
    fn a_newer_version_is_an_update() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.update_available(Some("1.26.4501.0")), Some(false));
        assert_eq!(installed.update_available(Some("1.26.4600.0")), Some(true));
        // Downgrades count as different, because "not what I have" is the
        // question; a rollback still needs the files fetched.
        assert_eq!(installed.update_available(Some("1.26.4403.0")), Some(true));
    }

    /// Neither "up to date" nor "stale" is honest with nothing to compare, and
    /// a launcher that guesses either way either nags or hides a real update.
    /// This is normal only while the update service is unavailable.
    #[test]
    fn nothing_to_compare_is_unknown_not_a_verdict() {
        let installed = record(&["content-a"]);
        assert_eq!(installed.update_available(None), None);
        assert_eq!(installed.update_available(Some("")), None);

        let mut versionless = installed.clone();
        versionless.package_version = None;
        assert_eq!(versionless.update_available(Some("1.26.4600.0")), None);
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
        let dir =
            std::env::temp_dir().join(format!("ferestre-container-test-{}", std::process::id()));
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
        let dir =
            std::env::temp_dir().join(format!("ferestre-container-neg-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("ferestre-adopt-test-{}", std::process::id()));
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

        // The point of all of it: an adopted record can answer the update
        // question, once something knows what version is on offer.
        assert_eq!(adopted.update_available(Some("1.26.4501.0")), Some(false));
        assert_eq!(adopted.update_available(Some("1.27.0.0")), Some(true));

        std::fs::remove_dir_all(&dir).expect("cleans up");
    }

    /// Asking someone to find an .exe in a tree of thousands is what this
    /// avoids. The manifest names it, so there is nothing to guess.
    #[test]
    fn the_executable_comes_from_the_manifest() {
        let dir = std::env::temp_dir().join(format!("ferestre-exe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");

        let write = |body: &str| {
            std::fs::write(dir.join("appxmanifest.xml"), body).expect("writes");
            executable(&dir)
        };

        assert_eq!(
            write(
                r#"<Package><Applications><Application Id="Game" Executable="Minecraft.Windows.exe" EntryPoint="Windows.FullTrustApplication" /></Applications></Package>"#
            ),
            Some("Minecraft.Windows.exe".into())
        );
        assert_eq!(
            write("<Package><Application Executable='Game.exe' Id=\"x\" /></Package>"),
            Some("Game.exe".into()),
            "attribute order and quoting are not guaranteed"
        );
        assert_eq!(
            write(
                r#"<Package><Application Executable="First.exe" /><Application Executable="Second.exe" /></Package>"#
            ),
            Some("First.exe".into()),
            "several applications: the first is the one the Store shows"
        );
        assert_eq!(write(r#"<Package><Application Id="x" /></Package>"#), None);
        assert_eq!(
            write(r#"<Package><Application Executable="" /></Package>"#),
            None
        );
        assert_eq!(write("not xml"), None);
        assert_eq!(
            write(r#"<Package><Dependency Executable="Wrong.exe" /></Package>"#),
            None,
            "only an <Application> names the entry point"
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
        let dir = std::env::temp_dir().join(format!("ferestre-stale-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");
        std::fs::write(dir.join(CONTAINER), b"header").expect("write");

        let mut present = record(&["content-a"]);
        present.dir = dir.clone();
        assert!(!present.is_stale());

        std::fs::remove_dir_all(&dir).expect("removes it");
        assert!(present.is_stale(), "the directory is gone");
    }

    /// And a record for a directory that is *there* but holds nothing. This is
    /// the shape a failed install leaves: the client printed "not entitled to
    /// this content", exited zero, and left a partial container behind. The
    /// record was written, and the title claimed to be installed from then on.
    #[test]
    fn a_record_for_a_directory_that_holds_no_install_is_stale() {
        let dir = std::env::temp_dir().join(format!("ferestre-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the dir");
        std::fs::write(dir.join(".xodus-streaming-tmp.msixvc"), b"partial").expect("write");

        let mut record = record(&[]);
        record.dir = dir.clone();
        assert!(
            record.is_stale(),
            "a directory is not an install just because it exists"
        );
        std::fs::remove_dir_all(&dir).ok();
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
        let dir =
            std::env::temp_dir().join(format!("ferestre-install-test-{}", std::process::id()));
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

    /// The service is bound to what the runtime will send, so this has to
    /// read the config the way the runtime does, quirks included.
    #[test]
    fn the_title_identity_is_read_the_way_the_runtime_reads_it() {
        let dir = std::env::temp_dir().join(format!("ferestre-title-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(title_identity(&dir), None);
        let identity = |config: &str| {
            std::fs::write(dir.join("MicrosoftGame.Config"), config).unwrap();
            title_identity(&dir).unwrap()
        };
        let expect = |title_id: &str, client_id: &str| TitleIdentity {
            title_id: title_id.into(),
            client_id: client_id.into(),
        };
        assert_eq!(
            identity(
                "<Game><TitleId>6B9DE498</TitleId><MSAAppId>00000000-0000-0000-0000-00004C26E6C9</MSAAppId></Game>"
            ),
            expect("1805509784", "00000000-0000-0000-0000-00004C26E6C9")
        );
        // A Windows title authenticates by TitleId alone.
        assert_eq!(
            identity("<Game><TitleId> 0x4d5f3c6a</TitleId></Game>"),
            expect("1298087018", "")
        );
        // Trailing text after the digits is ignored, as strtoul ignores it.
        assert_eq!(
            identity("<TitleId>7900C3C7 </TitleId>"),
            expect("2030093255", "")
        );
        for config in [
            "<Game><MSAAppId>client</MSAAppId></Game>",
            "<Game><TitleId></TitleId><MSAAppId>client</MSAAppId></Game>",
            "<TitleId>0</TitleId>",
            "<TitleId>not hex</TitleId>",
            "<TitleId>100000000</TitleId>",
            &format!("<TitleId>{}</TitleId>", "1".repeat(64)),
        ] {
            assert_eq!(identity(config), TitleIdentity::default(), "{config}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
