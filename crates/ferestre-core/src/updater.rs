//! Signed application and runtime updates. GitHub is discovery, not a trust key.
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{http, runtime, signing};

const API: &str = "https://api.github.com/repos/icex/ferestre/releases";
const DOWNLOAD: &str = "https://github.com/icex/ferestre/releases/download";
const FEED_LIMIT: u64 = 8 * 1024 * 1024;
const MANIFEST_LIMIT: u64 = 64 * 1024;

/// AppImage launchers export these variables to their descendants too. Only
/// use the image when Ferestre itself is executing from the matching AppDir.
pub fn current_image() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    associated_image(
        std::env::var_os("APPIMAGE").as_deref(),
        std::env::var_os("APPDIR").as_deref(),
        &executable,
    )
}

fn associated_image(
    image: Option<&std::ffi::OsStr>,
    appdir: Option<&std::ffi::OsStr>,
    executable: &Path,
) -> Option<PathBuf> {
    let program = crate::steam::shortcut_program(image, appdir, executable);
    if program == executable {
        return None;
    }
    let appdir = Path::new(appdir?);
    let relative = executable.strip_prefix(appdir).ok()?;
    let ours =
        relative == Path::new("usr/bin/ferestre") || relative == Path::new("usr/bin/ferestre-gui");
    (ours && appdir.join("usr/lib/ferestre/VERSION").is_file()).then_some(program)
}

/// AppRun honours explicit locator overrides. An updated image must find its
/// own binaries and VERSION rather than inherit the old mount's locators.
pub fn image_command(image: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(image);
    for name in [
        "APPDIR",
        "APPIMAGE",
        "ARGV0",
        "FERESTRE_PACKAGE_VERSION",
        "FERESTRE_SCRIPTS_DIR",
        "XODUS_CLI_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

/// Bind a later UI action to the executable present when this window opened.
pub fn image_fingerprint(path: &Path) -> Result<String> {
    let target = AppImageTarget::inspect(path)?;
    let digest = sha256(&target.path)?;
    anyhow::ensure!(
        AppImageTarget::inspect(path)?.identity == target.identity,
        "the installed AppImage changed while it was checked"
    );
    Ok(digest)
}

pub fn at_least(version: &str, minimum: &str, component: Component) -> Result<bool> {
    numbers(minimum)?;
    Ok(match component {
        Component::Launcher => current_launcher(version)? >= numbers(minimum)?,
        Component::Runtime => runtime::compare_versions(version, minimum).is_ge(),
    })
}

pub fn current_runtime_version(paths: &crate::paths::Paths) -> Option<String> {
    runtime::all(paths)
        .into_iter()
        .filter_map(|r| runtime::installed_version(&r, &paths.runtimes_dir()))
        .max_by(|a, b| runtime::compare_versions(a, b))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Launcher,
    Runtime,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Available {
    pub component: Component,
    pub version: String,
    pub tag: String,
    pub asset: String,
    pub url: String,
    pub signature_url: String,
    pub sha256: String,
}
impl Available {
    pub fn artefact(&self) -> signing::Artefact<'_> {
        signing::Artefact::new(&self.asset, &self.version)
    }
    pub fn runtime_release(&self) -> runtime::Release {
        runtime::Release {
            tag: self.tag.clone(),
            version: self.version.clone(),
            asset_name: self.asset.clone(),
            url: self.url.clone(),
            signature_url: Some(self.signature_url.clone()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub schema: u32,
    pub launcher: Option<Available>,
    pub runtime: Option<Available>,
    /// Only writable, user-owned AppImages are replaced by this updater.
    pub appimage: Option<PathBuf>,
    pub launcher_unavailable: Option<String>,
}
impl Status {
    pub fn available(&self) -> bool {
        self.launcher.is_some() || self.runtime.is_some()
    }
    pub fn actionable(&self) -> bool {
        (self.launcher.is_some() && self.appimage.is_some()) || self.runtime.is_some()
    }
    pub fn title(&self) -> String {
        match (&self.launcher, &self.runtime) {
            (Some(app), Some(rt)) => format!(
                "Ferestre {} and runtime {} are available",
                app.version, rt.version
            ),
            (Some(app), None) => format!("Ferestre {} is available", app.version),
            (None, Some(rt)) => format!("Runtime {} is available", rt.version),
            _ => "Ferestre is up to date".into(),
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn numbers(value: &str) -> Result<Vec<u64>> {
    let parts: Vec<&str> = value.split('.').collect();
    anyhow::ensure!(
        (3..=5).contains(&parts.len()),
        "invalid release version: {value}"
    );
    parts
        .into_iter()
        .map(|p| {
            anyhow::ensure!(
                !p.is_empty()
                    && p.bytes().all(|b| b.is_ascii_digit())
                    && (p == "0" || !p.starts_with('0')),
                "invalid release version: {value}"
            );
            p.parse()
                .map_err(|_| anyhow!("invalid release version: {value}"))
        })
        .collect()
}

fn current_launcher(value: &str) -> Result<Vec<u64>> {
    // Source/development builds may add .g<commit>, -<prerelease> or +<build>.
    let base = value.split(['-', '+']).next().unwrap_or(value);
    numbers(&base.split('.').take(3).collect::<Vec<_>>().join("."))
}

fn asset_url(release: &Release, name: &str) -> Result<String> {
    let expected = format!("{DOWNLOAD}/{}/{name}", release.tag_name);
    let mut matches = release.assets.iter().filter(|a| a.name == name);
    let found = matches
        .next()
        .ok_or_else(|| anyhow!("release {} has no {name}", release.tag_name))?;
    anyhow::ensure!(
        matches.next().is_none() && found.browser_download_url == expected,
        "release {} has an unexpected URL or duplicate asset for {name}",
        release.tag_name
    );
    Ok(expected)
}

fn candidates(text: &str, found: &mut [Option<Release>; 2]) -> Result<usize> {
    let releases: Vec<Release> =
        serde_json::from_str(text).context("the release feed is not a JSON list")?;
    let count = releases.len();
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        let (slot, version) = if let Some(v) = release.tag_name.strip_prefix("runtime-") {
            (1, v)
        } else if let Some(v) = release.tag_name.strip_prefix('v') {
            (0, v)
        } else {
            continue;
        };
        let Ok(version) = numbers(version) else {
            continue;
        };
        if slot == 0 && version.len() != 3 {
            continue;
        }
        let replace = found[slot].as_ref().is_none_or(|old| {
            let v = old
                .tag_name
                .trim_start_matches("runtime-")
                .trim_start_matches('v');
            version > numbers(v).unwrap_or_default()
        });
        if replace {
            found[slot] = Some(release);
        }
    }
    Ok(count)
}

fn checksum(manifest: &str, name: &str) -> Result<String> {
    let mut result = None;
    for line in manifest.lines().filter(|s| !s.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let digest = fields.next().unwrap_or_default();
        let file = fields.next().unwrap_or_default();
        let file = file.strip_prefix('*').unwrap_or(file);
        let file = file.strip_prefix("./").unwrap_or(file);
        anyhow::ensure!(
            fields.next().is_none()
                && digest.len() == 64
                && digest.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid signed checksum manifest"
        );
        if file == name {
            anyhow::ensure!(
                result.is_none(),
                "signed checksum manifest lists {name} more than once"
            );
            result = Some(digest.to_ascii_lowercase());
        }
    }
    result.ok_or_else(|| anyhow!("the signed checksum manifest does not name {name}"))
}

fn authenticate_release(
    release: Release,
    component: Component,
    fetch: &impl Fn(&str, u64) -> Result<String>,
    public_key: &str,
) -> Result<Available> {
    let version = match component {
        Component::Launcher => release.tag_name.strip_prefix('v'),
        Component::Runtime => release.tag_name.strip_prefix("runtime-"),
    }
    .unwrap()
    .to_string();
    let asset = match component {
        Component::Launcher => format!("ferestre-{version}-x86_64.AppImage"),
        Component::Runtime => runtime::archive_name(&version),
    };
    let url = asset_url(&release, &asset)?;
    let signature_url = asset_url(&release, &format!("{asset}.minisig"))?;
    let manifest = fetch(&asset_url(&release, "SHA256SUMS")?, MANIFEST_LIMIT)?;
    let signature = fetch(&asset_url(&release, "SHA256SUMS.minisig")?, 16 * 1024)?;
    signing::verify_bytes_with_key(
        public_key,
        &signature,
        signing::Artefact::new("SHA256SUMS", &version),
        manifest.as_bytes(),
    )?;
    let sha256 = checksum(&manifest, &asset)?;
    Ok(Available {
        component,
        version,
        tag: release.tag_name,
        asset,
        url,
        signature_url,
        sha256,
    })
}

pub fn check(
    paths: &crate::paths::Paths,
    current: &str,
    appimage: Option<&Path>,
) -> Result<Status> {
    check_cancellable(paths, current, appimage, || false)
}

pub fn check_cancellable(
    paths: &crate::paths::Paths,
    current: &str,
    appimage: Option<&Path>,
    cancelled: impl Fn() -> bool,
) -> Result<Status> {
    let newest = current_runtime_version(paths);
    let (appimage, launcher_unavailable) = match appimage {
        Some(path) => match AppImageTarget::inspect(path) {
            Ok(target) => (Some(target.path), None),
            Err(e) => (None, Some(e.to_string())),
        },
        None => (None, Some("Update this Ferestre installation with its package manager; automatic launcher updates support user-owned AppImages.".into())),
    };
    check_with(
        current,
        newest.as_deref(),
        appimage,
        launcher_unavailable,
        |page| {
            anyhow::ensure!(!cancelled(), "Update stopped");
            http::get_release_text(&format!("{API}?per_page=100&page={page}"), FEED_LIMIT)
        },
        |url, limit| {
            anyhow::ensure!(!cancelled(), "Update stopped");
            http::get_release_text(url, limit)
        },
        signing::RELEASE_PUBLIC_KEY,
    )
}

fn check_with(
    current: &str,
    runtime: Option<&str>,
    appimage: Option<PathBuf>,
    launcher_unavailable: Option<String>,
    page: impl Fn(u32) -> Result<String>,
    fetch: impl Fn(&str, u64) -> Result<String>,
    key: &str,
) -> Result<Status> {
    let current = current_launcher(current)?;
    let mut found = [None, None];
    for number in 1..=10 {
        let count = candidates(&page(number)?, &mut found)?;
        if count < 100 || found.iter().all(Option::is_some) {
            break;
        }
    }
    let [app, rt] = found;
    let launcher = app
        .filter(|r| numbers(r.tag_name.strip_prefix('v').unwrap()).unwrap() > current)
        .map(|r| authenticate_release(r, Component::Launcher, &fetch, key))
        .transpose()?;
    let runtime = rt
        .filter(|r| {
            runtime.is_none_or(|old| {
                crate::runtime::compare_versions(r.tag_name.trim_start_matches("runtime-"), old)
                    .is_gt()
            })
        })
        .map(|r| authenticate_release(r, Component::Runtime, &fetch, key))
        .transpose()?;
    Ok(Status {
        schema: 1,
        launcher,
        runtime,
        appimage,
        launcher_unavailable,
    })
}

pub fn verify_checksum(path: &Path, expected: &str) -> Result<()> {
    anyhow::ensure!(
        sha256(path)? == expected,
        "{} does not match its signed checksum",
        path.display()
    );
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppliedAppImage {
    pub target: PathBuf,
    pub backup: PathBuf,
    pub installed_sha256: String,
    pub previous_sha256: String,
}

/// Restore the exact previous image when restart could not be executed.
/// Refuse if either file was changed by another updater or the user.
pub fn rollback(update: &AppliedAppImage) -> Result<()> {
    let target = AppImageTarget::inspect(&update.target)?;
    let backup = AppImageTarget::inspect(&update.backup)?;
    anyhow::ensure!(
        target.path.parent() == backup.path.parent(),
        "the backup is not beside the AppImage"
    );
    let prefix = format!(".{}.", target.path.file_name().unwrap().to_string_lossy());
    let name = backup.path.file_name().unwrap().to_string_lossy();
    anyhow::ensure!(
        name.starts_with(&prefix) && name.ends_with(".previous"),
        "unexpected rollback filename"
    );
    let _lock = PreparedAppImage::reserve(target)?;
    rollback_locked(update)
}

fn rollback_locked(update: &AppliedAppImage) -> Result<()> {
    verify_checksum(&update.target, &update.installed_sha256)?;
    verify_checksum(&update.backup, &update.previous_sha256)?;
    std::fs::rename(&update.backup, &update.target)?;
    crate::cache::sync_directory(update.target.parent().unwrap());
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    ns: i64,
    mode: u32,
}
impl Identity {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.len(),
            mtime: meta.mtime(),
            ns: meta.mtime_nsec(),
            mode: meta.mode(),
        }
    }
}

pub struct AppImageTarget {
    pub path: PathBuf,
    identity: Identity,
}
impl AppImageTarget {
    pub fn inspect(path: &Path) -> Result<Self> {
        anyhow::ensure!(path.is_absolute(), "the AppImage path must be absolute");
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("the AppImage has no parent"))?
            .canonicalize()?;
        let path = parent.join(
            path.file_name()
                .ok_or_else(|| anyhow!("the AppImage has no filename"))?,
        );
        let meta = std::fs::symlink_metadata(&path)?;
        // SAFETY: geteuid has no preconditions and does not change process state.
        let uid = unsafe { libc::geteuid() };
        anyhow::ensure!(uid != 0, "do not self-update Ferestre as root");
        anyhow::ensure!(meta.is_file() && !meta.file_type().is_symlink() && meta.uid() == uid && meta.mode() & 0o500 == 0o500 && meta.mode() & 0o6022 == 0,
            "the AppImage must be a user-owned regular file without shared write or set-ID permissions");
        let dir = std::fs::metadata(&parent)?;
        anyhow::ensure!(
            dir.uid() == uid && dir.mode() & 0o022 == 0 && dir.mode() & 0o300 == 0o300,
            "the AppImage folder must be owned by you without shared write permissions"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        anyhow::ensure!(
            Identity::of(&file.metadata()?) == Identity::of(&meta),
            "the AppImage changed while it was checked"
        );
        let mut head = [0; 11];
        file.read_exact(&mut head)?;
        anyhow::ensure!(
            &head[..4] == b"\x7fELF" && &head[8..11] == b"AI\x02",
            "the update target is not a type-2 AppImage"
        );
        Ok(Self {
            path,
            identity: Identity::of(&meta),
        })
    }
}

/// All executable changes are staged beside the target, while holding a lock.
/// Download/verification/probe/cancellation failures leave the installed inode alone.
pub struct PreparedAppImage {
    target: AppImageTarget,
    staged: PathBuf,
    staged_inode: (u64, u64),
    expected_checksum: Option<String>,
    _lock: File,
}
impl Drop for PreparedAppImage {
    fn drop(&mut self) {
        if self.owns_stage() {
            let _ = std::fs::remove_file(&self.staged);
        }
    }
}
impl PreparedAppImage {
    pub fn prepare(
        target: &Path,
        release: &Available,
        cache: &Path,
        cancelled: impl Fn() -> bool,
        probe: impl Fn(&Path, &str) -> Result<()>,
    ) -> Result<Self> {
        let target = AppImageTarget::inspect(target)?;
        let mut prepared = Self::reserve(target)?;
        anyhow::ensure!(!cancelled(), "Update stopped");
        std::fs::create_dir_all(cache)?;
        let download = cache.join(&release.asset);
        http::download_signed_https(
            &release.url,
            &release.signature_url,
            release.artefact(),
            &download,
            &cancelled,
        )?;
        verify_checksum(&download, &release.sha256)?;
        prepared.copy_verified(&download)?;
        verify_checksum(&prepared.staged, &release.sha256)?;
        anyhow::ensure!(!cancelled(), "Update stopped");
        probe(&prepared.staged, &release.version)?;
        prepared.expected_checksum = Some(release.sha256.clone());
        anyhow::ensure!(!cancelled(), "Update stopped");
        Ok(prepared)
    }

    fn reserve(target: AppImageTarget) -> Result<Self> {
        let parent = target.path.parent().unwrap();
        let name = target.path.file_name().unwrap().to_string_lossy();
        let lock_path = parent.join(format!(".{name}.update.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock_path)?;
        let meta = lock.metadata()?;
        // SAFETY: descriptor belongs to `lock`; flock does not outlive it.
        anyhow::ensure!(
            meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "unsafe updater lock"
        );
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("another Ferestre update is already running");
        }
        let staged = crate::cache::temporary_beside(&target.path, "update");
        // Own the file only after exclusive creation succeeds. A collision
        // must never cause Drop to delete somebody else's file.
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)?;
        let metadata = file.metadata()?;
        Ok(Self {
            target,
            staged,
            staged_inode: (metadata.dev(), metadata.ino()),
            expected_checksum: None,
            _lock: lock,
        })
    }

    fn owns_stage(&self) -> bool {
        std::fs::symlink_metadata(&self.staged)
            .is_ok_and(|m| m.is_file() && (m.dev(), m.ino()) == self.staged_inode)
    }

    fn copy_verified(&mut self, source: &Path) -> Result<()> {
        let mut input = File::open(source)?;
        let mut out = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.staged)?;
        let meta = out.metadata()?;
        anyhow::ensure!(
            (meta.dev(), meta.ino()) == self.staged_inode,
            "the staged AppImage changed"
        );
        std::io::copy(&mut input, &mut out)?;
        out.set_permissions(std::fs::Permissions::from_mode(
            self.target.identity.mode & 0o755,
        ))?;
        out.flush()?;
        out.sync_all()?;
        Ok(())
    }

    pub fn apply(self) -> Result<AppliedAppImage> {
        self.apply_recorded(None)
    }

    pub fn apply_with_receipt(self, receipt: &Path) -> Result<AppliedAppImage> {
        self.apply_recorded(Some(receipt))
    }

    fn apply_recorded(self, receipt: Option<&Path>) -> Result<AppliedAppImage> {
        anyhow::ensure!(
            self.owns_stage(),
            "the staged AppImage changed; nothing was replaced"
        );
        verify_checksum(
            &self.staged,
            self.expected_checksum
                .as_deref()
                .ok_or_else(|| anyhow!("the staged AppImage has not been verified"))?,
        )?;
        let now = AppImageTarget::inspect(&self.target.path)?;
        anyhow::ensure!(
            now.identity == self.target.identity,
            "the installed AppImage changed during the download; nothing was replaced"
        );
        let backup = crate::cache::temporary_beside(&self.target.path, "previous");
        let previous_sha256 = sha256(&self.target.path)?;
        std::fs::hard_link(&self.target.path, &backup)
            .context("could not keep the previous AppImage")?;
        let applied = AppliedAppImage {
            target: self.target.path.clone(),
            backup: backup.clone(),
            installed_sha256: self.expected_checksum.clone().unwrap(),
            previous_sha256,
        };
        // Make recovery durable before replacing the current executable.
        // A crash before rename leaves the original executable in place;
        // a crash after it leaves this exact backup and receipt available.
        let recovery = (|| -> Result<()> {
            anyhow::ensure!(
                Identity::of(&std::fs::symlink_metadata(&backup)?) == self.target.identity,
                "the installed AppImage changed before its backup was saved"
            );
            File::open(self.target.path.parent().unwrap())?.sync_all()?;
            if let Some(path) = receipt {
                crate::cache::write(path, &serde_json::to_vec_pretty(&applied)?)?;
            }
            Ok(())
        })();
        if let Err(error) = recovery {
            let _ = std::fs::remove_file(&backup);
            return Err(error)
                .context("could not save update recovery data; the previous AppImage was kept");
        }
        if let Err(e) = std::fs::rename(&self.staged, &self.target.path) {
            let _ = std::fs::remove_file(&backup);
            return Err(e).context(
                "could not replace the AppImage; the previous version is still installed",
            );
        }
        if let Err(error) =
            File::open(self.target.path.parent().unwrap()).and_then(|dir| dir.sync_all())
        {
            rollback_locked(&applied).with_context(|| {
                format!("could not sync the update ({error}) or finish restoring its previous copy")
            })?;
            bail!("could not sync the update ({error}); the previous AppImage was restored");
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::testing::TestKey;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    struct Feed {
        key: TestKey,
        releases: Vec<Value>,
        files: BTreeMap<String, String>,
    }
    impl Feed {
        fn new() -> Self {
            Self {
                key: TestKey::new(),
                releases: vec![],
                files: BTreeMap::new(),
            }
        }
        fn release(&mut self, tag: &str) {
            let version = tag.trim_start_matches("runtime-").trim_start_matches('v');
            let asset = if tag.starts_with("runtime-") {
                runtime::archive_name(version)
            } else {
                format!("ferestre-{version}-x86_64.AppImage")
            };
            let names = [
                asset.clone(),
                format!("{asset}.minisig"),
                "SHA256SUMS".into(),
                "SHA256SUMS.minisig".into(),
            ];
            self.releases.push(json!({"tag_name":tag, "assets":names.iter().map(|name|
                json!({"name":name,"browser_download_url":format!("{DOWNLOAD}/{tag}/{name}")})).collect::<Vec<_>>()}));
            let manifest = format!("{:x}  ./{asset}\n", Sha256::digest(b"fixture bytes"));
            let signature = self.key.sign_as(
                signing::Artefact::new("SHA256SUMS", version),
                manifest.as_bytes(),
            );
            self.files
                .insert(format!("{DOWNLOAD}/{tag}/SHA256SUMS"), manifest);
            self.files
                .insert(format!("{DOWNLOAD}/{tag}/SHA256SUMS.minisig"), signature);
        }
        fn check(&self, current: &str, runtime: Option<&str>) -> Result<Status> {
            check_with(
                current,
                runtime,
                Some(PathBuf::from("/fixture.AppImage")),
                None,
                |_| Ok(serde_json::to_string(&self.releases)?),
                |url, _| {
                    self.files
                        .get(url)
                        .cloned()
                        .ok_or_else(|| anyhow!("unexpected fetch {url}"))
                },
                &self.key.public(),
            )
        }
    }

    #[test]
    fn both_components_need_authenticated_metadata_and_newer_versions() {
        let mut feed = Feed::new();
        feed.release("v0.1.16");
        feed.release("runtime-11.0.20261103.1");
        let result = feed
            .check("0.1.15", Some("ferestre-11.0.20261023.1"))
            .unwrap();
        assert_eq!(result.launcher.unwrap().version, "0.1.16");
        assert_eq!(result.runtime.unwrap().version, "11.0.20261103.1");
        let result = feed.check("0.1.16+build", Some("11.0.20261103.1")).unwrap();
        assert!(!result.available());
        assert!(!feed
            .check("0.1.17", Some("11.0.20261104.1"))
            .unwrap()
            .available());
        assert!(feed.check("unknown", None).is_err());
    }

    #[test]
    fn numeric_versions_win_over_feed_order_and_custom_build_suffixes() {
        let mut feed = Feed::new();
        feed.release("v0.1.9");
        feed.release("v0.1.16");
        feed.release("v0.1.15");
        assert_eq!(
            feed.check("0.1.15-authfix.1", None)
                .unwrap()
                .launcher
                .unwrap()
                .version,
            "0.1.16"
        );
        assert_eq!(
            feed.check("0.1.15.gabc123", None)
                .unwrap()
                .launcher
                .unwrap()
                .version,
            "0.1.16"
        );
        for invalid in [
            "01.2.3",
            "1.2",
            "1.2.3/evil",
            "1.2.3.4.5.6",
            "18446744073709551616.0.0",
        ] {
            assert!(numbers(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn unsigned_mismatched_or_damaged_metadata_is_never_offered() {
        for scenario in 0..5 {
            let mut feed = Feed::new();
            feed.release("v0.1.16");
            let base = format!("{DOWNLOAD}/v0.1.16/");
            match scenario {
                0 => {
                    feed.files
                        .get_mut(&(base.clone() + "SHA256SUMS"))
                        .unwrap()
                        .push('x');
                }
                1 => {
                    feed.files
                        .insert(base.clone() + "SHA256SUMS.minisig", "invalid".into());
                }
                2 => {
                    let other = TestKey::new();
                    let manifest = &feed.files[&(base.clone() + "SHA256SUMS")];
                    feed.files.insert(
                        base.clone() + "SHA256SUMS.minisig",
                        other.sign_as(
                            signing::Artefact::new("SHA256SUMS", "0.1.16"),
                            manifest.as_bytes(),
                        ),
                    );
                }
                3 => {
                    let manifest = &feed.files[&(base.clone() + "SHA256SUMS")];
                    feed.files.insert(
                        base.clone() + "SHA256SUMS.minisig",
                        feed.key.sign_as(
                            signing::Artefact::new("SHA256SUMS", "0.1.15"),
                            manifest.as_bytes(),
                        ),
                    );
                }
                4 => {
                    feed.releases[0]["assets"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|a| a["name"] != "SHA256SUMS.minisig");
                }
                _ => unreachable!(),
            }
            assert!(feed.check("0.1.15", None).is_err(), "scenario {scenario}");
        }
    }

    #[test]
    fn release_urls_must_exactly_name_the_repository_tag_and_asset() {
        for url in ["http://github.com/icex/ferestre/releases/download/v0.1.16/x", "https://attacker.test/file", "https://github.com/another/repo/releases/download/v0.1.16/x", "https://github.com/icex/ferestre/releases/download/v0.1.16/ferestre-0.1.16-x86_64.AppImage?token=x"] {
            let mut feed=Feed::new();feed.release("v0.1.16");
            feed.releases[0]["assets"][0]["browser_download_url"]=json!(url);
            assert!(feed.check("0.1.15",None).is_err(),"{url}");
        }
        let mut feed = Feed::new();
        feed.release("v0.1.16");
        let duplicate = feed.releases[0]["assets"][0].clone();
        feed.releases[0]["assets"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(feed.check("0.1.15", None).is_err());
    }

    #[test]
    fn applying_an_offer_cannot_silently_fall_back_to_an_older_release() {
        assert!(!at_least("0.1.15", "0.1.16", Component::Launcher).unwrap());
        assert!(at_least("0.1.17", "0.1.16", Component::Launcher).unwrap());
        assert!(at_least("0.1.16+build", "0.1.16", Component::Launcher).unwrap());
        assert!(!at_least("11.0.20261018.2", "11.0.20261023.1", Component::Runtime).unwrap());
        assert!(at_least(
            "ferestre-11.0.20261023.1",
            "11.0.20261023.1",
            Component::Runtime
        )
        .unwrap());
        assert!(at_least("0.1.17", "../unsafe", Component::Launcher).is_err());
    }

    #[test]
    fn draft_and_prerelease_builds_are_not_automatic_updates() {
        let mut feed = Feed::new();
        feed.release("v0.1.16");
        feed.release("v0.1.99");
        feed.release("runtime-99.0.1");
        feed.releases[1]["draft"] = json!(true);
        feed.releases[2]["prerelease"] = json!(true);
        let result = feed.check("0.1.15", None).unwrap();
        assert_eq!(result.launcher.unwrap().version, "0.1.16");
        assert!(result.runtime.is_none());
    }

    #[test]
    fn checks_look_past_a_full_page_without_runtime_releases() {
        let mut feed = Feed::new();
        feed.release("v0.1.16");
        feed.release("runtime-11.0.20261103.1");
        let calls = std::cell::Cell::new(0);
        let status = check_with(
            "0.1.15",
            Some("11.0.1"),
            None,
            None,
            |page| {
                calls.set(calls.get() + 1);
                if page == 1 {
                    Ok(serde_json::to_string(&vec![
                        json!({"tag_name":"unrelated","assets":[]});
                        100
                    ])?)
                } else {
                    Ok(serde_json::to_string(&feed.releases)?)
                }
            },
            |url, _| Ok(feed.files[url].clone()),
            &feed.key.public(),
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert!(status.runtime.is_some());
    }

    #[test]
    fn checksum_manifest_requires_one_exact_digest() {
        let hash = "ab".repeat(32);
        assert_eq!(
            checksum(&format!("{hash}  ./app.AppImage\n"), "app.AppImage").unwrap(),
            hash
        );
        assert_eq!(
            checksum(&format!("{hash} *app.AppImage\n"), "app.AppImage").unwrap(),
            hash
        );
        for bad in [
            format!("{hash} app.AppImage\n{hash} app.AppImage\n"),
            format!("{hash} app.AppImage extra"),
            format!("{hash} other.AppImage"),
            "garbage app.AppImage".into(),
        ] {
            assert!(checksum(&bad, "app.AppImage").is_err());
        }
    }

    fn image(dir: &Path, name: &str, payload: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = vec![0u8; 32];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[8..11].copy_from_slice(b"AI\x02");
        bytes.extend_from_slice(payload);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    fn prepared(path: &Path, source: &Path) -> PreparedAppImage {
        let mut staged = PreparedAppImage::reserve(AppImageTarget::inspect(path).unwrap()).unwrap();
        staged.copy_verified(source).unwrap();
        staged.expected_checksum = Some(sha256(source).unwrap());
        staged
    }

    #[test]
    fn replacement_is_atomic_keeps_old_open_files_and_can_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let new = image(dir.path(), "new.AppImage", b"new");
        let before = std::fs::read(&old).unwrap();
        let mut open = File::open(&old).unwrap();
        let staged = prepared(&old, &new);
        let path = staged.staged.clone();
        let result = staged.apply().unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), std::fs::read(&new).unwrap());
        assert_eq!(std::fs::read(&result.backup).unwrap(), before);
        assert!(!path.exists());
        let mut still_old = Vec::new();
        open.read_to_end(&mut still_old).unwrap();
        assert_eq!(still_old, before);
        rollback(&result).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), before);
        assert!(!result.backup.exists());
    }

    #[test]
    fn failed_or_cancelled_preparation_never_replaces_the_installed_image() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let original = sha256(&old).unwrap();
        let staged = PreparedAppImage::reserve(AppImageTarget::inspect(&old).unwrap()).unwrap();
        let path = staged.staged.clone();
        assert!(staged.apply().is_err());
        assert!(!path.exists());
        assert_eq!(sha256(&old).unwrap(), original);
        let release = Available {
            component: Component::Launcher,
            version: "0.1.16".into(),
            tag: "v0.1.16".into(),
            asset: "app.AppImage".into(),
            url: "https://example.invalid/app".into(),
            signature_url: "https://example.invalid/sig".into(),
            sha256: "00".repeat(32),
        };
        assert!(PreparedAppImage::prepare(
            &old,
            &release,
            &dir.path().join("cache"),
            || true,
            |_, _| panic!("cancelled update must not execute anything")
        )
        .is_err());
        assert_eq!(sha256(&old).unwrap(), original);
        assert!(!dir.path().join("cache").exists());
    }

    #[test]
    fn concurrent_updates_and_changes_during_download_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let new = image(dir.path(), "new.AppImage", b"new");
        let first = prepared(&old, &new);
        assert!(PreparedAppImage::reserve(AppImageTarget::inspect(&old).unwrap()).is_err());
        let external = image(dir.path(), "external.AppImage", b"independent update");
        std::fs::rename(&external, &old).unwrap();
        let expected = sha256(&old).unwrap();
        assert!(first.apply().is_err());
        assert_eq!(sha256(&old).unwrap(), expected);
        let next = prepared(&old, &new);
        std::fs::write(&next.staged, b"tampered").unwrap();
        assert!(next.apply().is_err());
        assert_eq!(sha256(&old).unwrap(), expected);
    }

    #[test]
    fn stage_cleanup_does_not_remove_a_file_it_did_not_create() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let staged = PreparedAppImage::reserve(AppImageTarget::inspect(&old).unwrap()).unwrap();
        let path = staged.staged.clone();
        let _keep_inode = File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"somebody else's file").unwrap();
        drop(staged);
        assert_eq!(std::fs::read(&path).unwrap(), b"somebody else's file");
    }

    #[test]
    fn symlinks_shared_writes_and_non_appimages_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let link = dir.path().join("alias.AppImage");
        std::os::unix::fs::symlink(&old, &link).unwrap();
        assert!(AppImageTarget::inspect(&link).is_err());
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(AppImageTarget::inspect(&old).is_err());
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(AppImageTarget::inspect(&old).is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&old, b"not an image").unwrap();
        assert!(AppImageTarget::inspect(&old).is_err());
    }

    #[test]
    fn rollback_refuses_a_later_independent_update() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let new = image(dir.path(), "new.AppImage", b"new");
        let result = prepared(&old, &new).apply().unwrap();
        let later = image(dir.path(), "later.AppImage", b"later");
        std::fs::rename(&later, &old).unwrap();
        let expected = sha256(&old).unwrap();
        assert!(rollback(&result).is_err());
        assert_eq!(sha256(&old).unwrap(), expected);
        assert!(result.backup.exists());
    }

    #[test]
    fn recovery_receipt_is_saved_before_replacement_and_can_restore() {
        let dir = tempfile::tempdir().unwrap();
        let old = image(dir.path(), "Ferestre.AppImage", b"old");
        let new = image(dir.path(), "new.AppImage", b"new");
        let receipt = dir.path().join("state/last-update.json");
        let applied = prepared(&old, &new).apply_with_receipt(&receipt).unwrap();
        let stored: AppliedAppImage =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        assert_eq!(stored.installed_sha256, applied.installed_sha256);
        assert_eq!(std::fs::metadata(&receipt).unwrap().mode() & 0o777, 0o600);
        rollback(&stored).unwrap();
        let original = sha256(&old).unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"keep").unwrap();
        assert!(prepared(&old, &new)
            .apply_with_receipt(&blocker.join("receipt"))
            .is_err());
        assert_eq!(sha256(&old).unwrap(), original);
    }

    #[test]
    fn managed_release_identity_uses_its_install_directory() {
        let managed = Path::new("/fixture/runtimes");
        let mut runtime = runtime::InstalledRuntime {
            path: managed.join("11.0.20261023.1"),
            version: Some("xodus-bleeding-edge-11.0-20260803-3-g7c0b4354".into()),
            provides: Default::default(),
            source: runtime::CapabilitySource::Manifest,
        };
        assert_eq!(
            runtime::installed_version(&runtime, managed).unwrap(),
            "11.0.20261023.1"
        );
        runtime.path = PathBuf::from("/other/custom-runtime");
        assert_eq!(
            runtime::installed_version(&runtime, managed),
            runtime.version
        );
    }

    #[test]
    fn an_inherited_appimage_cannot_replace_another_application() {
        let dir = tempfile::tempdir().unwrap();
        let other = image(dir.path(), "Other.AppImage", b"another application");
        let foreign_root = dir.path().join("AppDir");
        std::fs::create_dir_all(foreign_root.join("usr/lib/ferestre")).unwrap();
        std::fs::write(foreign_root.join("usr/lib/ferestre/VERSION"), b"0.1.15\n").unwrap();
        let foreign_mount = foreign_root.as_path();
        let native = Path::new("/usr/bin/ferestre");
        assert!(associated_image(
            Some(other.as_os_str()),
            Some(foreign_mount.as_os_str()),
            native
        )
        .is_none());
        assert!(associated_image(Some(other.as_os_str()), None, native).is_none());
        let embedded_helper = foreign_mount.join("usr/share/editor/ferestre");
        assert!(associated_image(
            Some(other.as_os_str()),
            Some(foreign_mount.as_os_str()),
            &embedded_helper
        )
        .is_none());
        let ours = foreign_mount.join("usr/bin/ferestre");
        assert_eq!(
            associated_image(
                Some(other.as_os_str()),
                Some(foreign_mount.as_os_str()),
                &ours
            ),
            Some(other)
        );
    }

    #[test]
    fn release_metadata_refuses_plaintext_before_connecting() {
        let error = http::get_release_text("http://127.0.0.1:9/manifest", 1024).unwrap_err();
        assert!(error.to_string().contains("https only"), "{error}");
    }
}
