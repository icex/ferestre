//! The DLSSNR-AMD Linux package: the release asset Ferestre builds from a
//! pinned upstream commit (`scripts/build-dlssnr.sh`) and hosts on a
//! `dlssnr-<version>` tag, or a locally built one named by
//! [`LOCAL_ENV`].
//!
//! The trust anchor is the SHA-256 of the published asset, pinned here; a
//! download that does not match is never kept. Unpacked once into
//! `<root>/packages/<version>-<arch>`, stamped with the asset's SHA-256.

use super::{Arch, Expect, Route};
use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where a package comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Url(String),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub version: String,
    pub arch: Arch,
    pub sha256: String,
    pub origin: Origin,
}

/// The directory naming a locally built package (the output directory of
/// `scripts/build-dlssnr.sh`), used instead of the pinned release.
pub const LOCAL_ENV: &str = "FERESTRE_DLSSNR_PACKAGE_DIR";

/// The upstream release the pinned assets were built from.
const PINNED_VERSION: &str = "0.0.9";
/// `(url, sha256)` of the published `dlssnr-0.0.9` assets, checked against
/// the release's signed `SHA256SUMS`.
const PINNED_X86_64: (&str, &str) = (
    "https://github.com/icex/ferestre/releases/download/dlssnr-0.0.9/ferestre-dlssnr-0.0.9-x86_64.tar.gz",
    "629bc699c9437c61e336be56a255000da9981b2d6a72178986a8aafa9c3a311e",
);
const PINNED_I686: (&str, &str) = (
    "https://github.com/icex/ferestre/releases/download/dlssnr-0.0.9/ferestre-dlssnr-0.0.9-i686.tar.gz",
    "ede9787f4ca679b546b2e4dff3f2a4f36a4d7c686a714a00ade54483c263fdf1",
);

const STAMP: &str = ".ferestre-sha256";
/// A package is never this large; a wrong URL that streams forever stops here.
const MAX_PACKAGE: u64 = 1024 * 1024 * 1024;

fn pinned_asset(arch: Arch) -> (&'static str, &'static str) {
    match arch {
        Arch::X86_64 => PINNED_X86_64,
        Arch::I686 => PINNED_I686,
    }
}

/// The release asset's file name.
pub fn asset_name(version: &str, arch: Arch) -> String {
    format!("ferestre-dlssnr-{version}-{arch}.tar.gz")
}

/// The pinned release asset, or `None` while nothing is pinned.
pub fn pinned(arch: Arch) -> Option<Package> {
    let (url, sha256) = pinned_asset(arch);
    if url.is_empty() || sha256.is_empty() {
        return None;
    }
    Some(Package {
        version: PINNED_VERSION.to_string(),
        arch,
        sha256: sha256.to_string(),
        origin: Origin::Url(url.to_string()),
    })
}

/// A version as it may appear in a path: no separators, no leading dot.
fn plain_version(version: &str) -> bool {
    !version.is_empty()
        && !version.starts_with('.')
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

/// The one `ferestre-dlssnr-<version>-<arch>.tar.gz` in `dir`.
pub fn local(dir: &Path, arch: Arch) -> Result<Package> {
    let suffix = format!("-{arch}.tar.gz");
    let entries = std::fs::read_dir(dir).with_context(|| format!("{}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(version) = name
            .strip_prefix("ferestre-dlssnr-")
            .and_then(|rest| rest.strip_suffix(&suffix))
        else {
            continue;
        };
        if plain_version(version) && entry.path().is_file() {
            found.push((version.to_string(), entry.path()));
        }
    }
    let (version, path) = match found.len() {
        1 => found.remove(0),
        0 => bail!(
            "{}: no {} package (ferestre-dlssnr-<version>{suffix}); build one with scripts/build-dlssnr.sh",
            dir.display(),
            arch
        ),
        _ => bail!(
            "{}: more than one {arch} package; keep exactly one ferestre-dlssnr-<version>{suffix}",
            dir.display()
        ),
    };
    let sha256 = crate::updater::sha256_file(&path)?;
    Ok(Package {
        version,
        arch,
        sha256,
        origin: Origin::File(path),
    })
}

/// The package to use: a local build named by [`LOCAL_ENV`] first, else the
/// pinned release.
pub fn current(paths: &Paths, arch: Arch) -> Result<Package> {
    if let Some(dir) = paths.var(LOCAL_ENV) {
        return local(Path::new(dir), arch);
    }
    pinned(arch).ok_or_else(|| {
        anyhow!(
            "no DLSSNR-AMD {arch} package is published for this Ferestre yet; build one with \
             scripts/build-dlssnr.sh and set {LOCAL_ENV} to its output directory"
        )
    })
}

fn package_dir(root: &Path, package: &Package) -> PathBuf {
    root.join("packages")
        .join(format!("{}-{}", package.version, package.arch))
}

fn complete(dir: &Path) -> bool {
    dir.join("install.sh").is_file() && dir.join("model-tools/extract_model.sh").is_file()
}

/// The package's directory if a complete copy of exactly this asset is there.
pub fn installed(root: &Path, package: &Package) -> Option<PathBuf> {
    let dir = package_dir(root, package);
    let stamp = std::fs::read_to_string(dir.join(STAMP)).ok()?;
    (stamp.trim() == package.sha256 && complete(&dir)).then_some(dir)
}

/// The unpacked directory of a package version, whichever asset it came
/// from, if it is complete: what an existing install is removed with.
pub fn unpacked(root: &Path, version: &str, arch: Arch) -> Option<PathBuf> {
    if !plain_version(version) {
        return None;
    }
    let dir = root.join("packages").join(format!("{version}-{arch}"));
    complete(&dir).then_some(dir)
}

/// The package's directory, downloading and unpacking it first if needed.
pub fn ensure(root: &Path, package: &Package) -> Result<PathBuf> {
    if !plain_version(&package.version) {
        bail!("{:?} is not a package version", package.version);
    }
    if let Some(dir) = installed(root, package) {
        return Ok(dir);
    }
    match &package.origin {
        Origin::File(file) => {
            crate::updater::verify_checksum(file, &package.sha256)
                .with_context(|| format!("{} changed since it was chosen", file.display()))?;
            unpack(root, package, file)
        }
        Origin::Url(url) => {
            let archive = root
                .join("downloads")
                .join(asset_name(&package.version, package.arch));
            if crate::updater::verify_checksum(&archive, &package.sha256).is_err() {
                let sha256 = package.sha256.clone();
                crate::http::download_large_checked(
                    url,
                    &archive,
                    MAX_PACKAGE,
                    || false,
                    move |path| {
                        crate::updater::verify_checksum(path, &sha256)
                            .map_err(|_| anyhow!("the download does not match its pinned SHA-256"))
                    },
                )?;
            }
            let result = unpack(root, package, &archive);
            let _ = std::fs::remove_file(&archive);
            result
        }
    }
}

fn unpack(root: &Path, package: &Package, archive: &Path) -> Result<PathBuf> {
    let path = package_dir(root, package);
    let parent = path.parent().expect("a package directory has a parent");
    std::fs::create_dir_all(parent).with_context(|| format!("{}", parent.display()))?;
    // Left by runs that were killed part way.
    for extension in ["unpack", "old"] {
        crate::cache::sweep_stale_temporaries(&path, extension);
    }
    // Unpacked beside its final place and renamed in, so a launch running at
    // the same time sees either no package or a whole one.
    let staging = crate::cache::temporary_beside(&path, "unpack");
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> Result<PathBuf> {
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(&staging)
            .args(["--no-same-owner", "--no-same-permissions"])
            .status()
            .context("cannot run tar to unpack the DLSSNR-AMD package")?;
        if !status.success() {
            bail!("tar could not unpack {} ({status})", archive.display());
        }
        if !complete(&staging) {
            bail!(
                "{} is not a DLSSNR-AMD package: it has no install.sh or model-tools/extract_model.sh",
                archive.display()
            );
        }
        std::fs::write(staging.join(STAMP), &package.sha256)?;
        if let Some(done) = installed(root, package) {
            // Another run finished first. Keep its copy: a game it set up
            // may already be using it.
            return Ok(done);
        }
        if path.exists() {
            // An incomplete copy or another asset of the same version.
            let old = crate::cache::temporary_beside(&path, "old");
            std::fs::rename(&path, &old)
                .with_context(|| format!("cannot replace {}", path.display()))?;
            let _ = std::fs::remove_dir_all(&old);
        }
        if let Err(e) = std::fs::rename(&staging, &path) {
            if installed(root, package).is_none() {
                return Err(e).with_context(|| format!("{}", path.display()));
            }
        }
        Ok(path.clone())
    })();
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// A top-level name a route writes into the game's executable folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFile {
    pub name: String,
    pub dir: bool,
    /// The package file or folder `install.sh` copies there; `None` for what
    /// the install or the game writes (manifest, ini, logs).
    pub source: Option<PathBuf>,
    /// Written only when the game has no file of that name.
    pub only_if_absent: bool,
}

fn written(name: &str, dir: bool) -> RouteFile {
    RouteFile {
        name: name.to_string(),
        dir,
        source: None,
        only_if_absent: false,
    }
}

fn copied(name: &str, source: PathBuf) -> RouteFile {
    RouteFile {
        name: name.to_string(),
        dir: source.is_dir(),
        source: Some(source),
        only_if_absent: false,
    }
}

/// Add or replace (a later copy of the same name wins, as in `install.sh`).
fn put(files: &mut Vec<RouteFile>, file: RouteFile) {
    match files.iter_mut().find(|f| f.name == file.name) {
        Some(slot) => *slot = file,
        None => files.push(file),
    }
}

fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut list = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("{}", dir.display()))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|n| anyhow!("{}: a name that is not UTF-8: {n:?}", dir.display()))?;
        list.push((name, entry.path()));
    }
    list.sort();
    Ok(list)
}

/// Every top-level name `install.sh` (at the pin) writes into the game's
/// folder for `route`, computed from the unpacked package rather than a
/// hardcoded list.
pub fn route_files(package_dir: &Path, route: Route) -> Result<Vec<RouteFile>> {
    let mut files = vec![
        written("dlssnr-amd-install.txt", false),
        copied("dlssnr-amd", package_dir.join("dlssnr-amd")),
        written("dlssnr-amd.ini", false),
        written("dlssnr-amd.log", false),
    ];
    // `dlssnr-amd/` is a folder whether or not this package has one yet.
    files[1].dir = true;
    match route {
        Route::Optiscaler => {
            let release = package_dir.join("optiscaler/release");
            if !release.join("OptiScaler.dll").is_file() {
                bail!("this package has no OptiScaler route (it is only in the 64-bit package)");
            }
            put(
                &mut files,
                copied("dxgi.dll", release.join("OptiScaler.dll")),
            );
            put(
                &mut files,
                copied("OptiScaler.ini", release.join("OptiScaler.ini")),
            );
            let mut folder = copied("OptiScaler", release.join("OptiScaler"));
            folder.dir = true;
            put(&mut files, folder);
            put(&mut files, written("OptiScaler.log", false));
            let mut vulkan = copied("vulkan-1.dll", package_dir.join("optiscaler/vulkan-1.dll"));
            vulkan.dir = false;
            vulkan.only_if_absent = true;
            put(&mut files, vulkan);
        }
        Route::ReshadeFeeder => {
            for (name, path) in entries(&package_dir.join("reshade"))? {
                put(&mut files, copied(&name, path));
            }
            put(&mut files, written("ReShade.log", false));
        }
        Route::ReshadeFeederVulkan | Route::ReshadeFeederDx9 => {
            let vulkan = package_dir.join("vulkan");
            let mut all = entries(&package_dir.join("reshade"))?;
            all.extend(entries(&vulkan)?);
            for (name, path) in all {
                if name == "dxgi.dll" || name == "ReShadePreset-d3d9.ini" {
                    continue;
                }
                put(&mut files, copied(&name, path));
            }
            if route == Route::ReshadeFeederDx9 {
                // Written over the route's own preset, unrecorded by the
                // manifest, so it has to be named here to be backed up.
                put(
                    &mut files,
                    copied("ReShadePreset.ini", vulkan.join("ReShadePreset-d3d9.ini")),
                );
            }
            put(&mut files, written("ReShade.log", false));
        }
    }
    Ok(files)
}

/// The names OptiScaler loads as (its own list), `dxgi` the default. A game
/// that crashes with a `dxgi.dll` beside it may take another.
pub const PROXIES: &[&str] = &[
    "dxgi", "winmm", "version", "dbghelp", "d3d12", "wininet", "winhttp",
];

/// `name` (with or without `.dll`, any case) as one of [`PROXIES`].
pub fn check_proxy(name: &str) -> Result<&'static str> {
    let lower = name.trim().to_ascii_lowercase();
    let bare = lower.strip_suffix(".dll").unwrap_or(&lower);
    PROXIES.iter().copied().find(|p| *p == bare).ok_or_else(|| {
        anyhow!(
            "{name:?} is not a name OptiScaler loads as; one of {}",
            PROXIES.join(", ")
        )
    })
}

/// The OptiScaler injector's file name for a proxy choice.
pub fn proxy_file(route: Route, proxy: Option<&str>) -> String {
    match proxy {
        Some(p) if route == Route::Optiscaler && p != "dxgi" => format!("{p}.dll"),
        _ => "dxgi.dll".to_string(),
    }
}

/// [`route_files`] with OptiScaler under another DLL name. `dxgi.dll` stays
/// listed: `install.sh` writes it before it is renamed, so a game's own is
/// still backed up and comes back.
pub fn route_files_as(
    package_dir: &Path,
    route: Route,
    proxy: Option<&str>,
) -> Result<Vec<RouteFile>> {
    let mut files = route_files(package_dir, route)?;
    let name = proxy_file(route, proxy);
    if name != "dxgi.dll" {
        if let Some(injector) = files.iter_mut().find(|f| f.name == "dxgi.dll") {
            injector.name = name;
        }
        put(&mut files, written("dxgi.dll", false));
    }
    Ok(files)
}

/// The files a route loads through: what `verify` compares with the package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Injectors {
    /// Always written by the route.
    pub required: &'static [&'static str],
    /// Compared only when the install manifest lists them (written only when
    /// the game has none of its own).
    pub when_listed: &'static [&'static str],
}

pub fn injectors(route: Route) -> Injectors {
    match route {
        Route::Optiscaler => Injectors {
            required: &["dxgi.dll"],
            when_listed: &["vulkan-1.dll"],
        },
        Route::ReshadeFeeder => Injectors {
            required: &["dxgi.dll"],
            when_listed: &[],
        },
        Route::ReshadeFeederVulkan | Route::ReshadeFeederDx9 => Injectors {
            required: &["vulkan-1.dll", "winevulkan.dll"],
            when_listed: &[],
        },
    }
}

/// Whether the package's `install.sh` checks for the same models Ferestre
/// caches. A package built from another upstream release may not.
pub fn expects_models(package_dir: &Path, expect: &Expect) -> bool {
    let Ok(script) = std::fs::read_to_string(package_dir.join("install.sh")) else {
        return false;
    };
    let model = format!("model_sha256={}", expect.aardwold.sha256);
    let small = format!("small_sha256={}", expect.small.sha256);
    let has = |wanted: &str| script.lines().any(|line| line.trim() == wanted);
    has(&model) && has(&small)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The expected prefix of every pinned URL: a release asset of Ferestre's.
    const RELEASE_PREFIX: &str = "https://github.com/icex/ferestre/releases/download/dlssnr-";

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A tarball shaped like upstream's: its files at the top, no folder.
    fn tarball(dest: &Path, files: &[(&str, &[u8])]) {
        let src = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            write(&src.path().join(name), bytes);
        }
        let ok = Command::new("tar")
            .arg("-czf")
            .arg(dest)
            .arg("-C")
            .arg(src.path())
            .arg(".")
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    fn package(archive: &Path) -> Package {
        Package {
            version: "0.0.9".into(),
            arch: Arch::X86_64,
            sha256: crate::updater::sha256_file(archive).unwrap(),
            origin: Origin::File(archive.to_path_buf()),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_package_unpacks_without_a_top_level_folder_and_is_stamped() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let archive = src.path().join(asset_name("0.0.9", Arch::X86_64));
        tarball(
            &archive,
            &[
                ("install.sh", b"#!/bin/bash\n"),
                ("model-tools/extract_model.sh", b"#!/bin/bash\n"),
                ("reshade/dxgi.dll", b"MZ"),
            ],
        );
        let pkg = package(&archive);
        let dir = ensure(root.path(), &pkg).unwrap();
        assert_eq!(dir, root.path().join("packages/0.0.9-x86_64"));
        assert_eq!(installed(root.path(), &pkg), Some(dir.clone()));
        assert!(dir.join("reshade/dxgi.dll").is_file());
        assert_eq!(names(&root.path().join("packages")), vec!["0.0.9-x86_64"]);
        assert!(
            archive.is_file(),
            "a local package is unpacked in place, not moved"
        );
        // Again: nothing to do.
        assert_eq!(ensure(root.path(), &pkg).unwrap(), dir);
    }

    #[test]
    fn a_tarball_missing_install_sh_is_refused_and_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let archive = src.path().join(asset_name("0.0.9", Arch::X86_64));
        tarball(
            &archive,
            &[("model-tools/extract_model.sh", b"#!/bin/bash\n")],
        );
        let pkg = package(&archive);
        let error = ensure(root.path(), &pkg).unwrap_err();
        assert!(format!("{error:#}").contains("install.sh"), "{error:#}");
        assert_eq!(installed(root.path(), &pkg), None);
        assert!(names(&root.path().join("packages")).is_empty());
    }

    #[test]
    fn a_local_file_that_changed_since_it_was_chosen_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let archive = src.path().join(asset_name("0.0.9", Arch::X86_64));
        tarball(
            &archive,
            &[
                ("install.sh", b"#!/bin/bash\n"),
                ("model-tools/extract_model.sh", b"#!/bin/bash\n"),
            ],
        );
        let mut pkg = package(&archive);
        pkg.sha256 = "0".repeat(64);
        assert!(ensure(root.path(), &pkg).is_err());
        assert!(!root.path().join("packages").exists());
    }

    #[test]
    fn a_copy_from_another_tarball_is_not_installed() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("packages/0.0.9-x86_64");
        write(&dir.join("install.sh"), b"x");
        write(&dir.join("model-tools/extract_model.sh"), b"x");
        write(&dir.join(STAMP), b"0000");
        let pkg = Package {
            version: "0.0.9".into(),
            arch: Arch::X86_64,
            sha256: "ab".repeat(32),
            origin: Origin::Url(String::new()),
        };
        assert_eq!(installed(root.path(), &pkg), None);
        write(&dir.join(STAMP), pkg.sha256.as_bytes());
        assert_eq!(installed(root.path(), &pkg), Some(dir.clone()));
        std::fs::remove_file(dir.join("model-tools/extract_model.sh")).unwrap();
        assert_eq!(installed(root.path(), &pkg), None, "an incomplete copy");
    }

    #[test]
    fn a_local_package_dir_names_its_version_and_arch() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("ferestre-dlssnr-0.0.9-x86_64.tar.gz"),
            b"tarball",
        );
        write(
            &dir.path().join("ferestre-dlssnr-0.0.9-i686.tar.gz"),
            b"other",
        );
        write(&dir.path().join("SHA256SUMS"), b"");
        let pkg = local(dir.path(), Arch::X86_64).unwrap();
        assert_eq!(pkg.version, "0.0.9");
        assert_eq!(pkg.arch, Arch::X86_64);
        assert_eq!(
            pkg.sha256,
            crate::updater::sha256_file(&dir.path().join("ferestre-dlssnr-0.0.9-x86_64.tar.gz"))
                .unwrap()
        );
        assert_eq!(
            pkg.origin,
            Origin::File(dir.path().join("ferestre-dlssnr-0.0.9-x86_64.tar.gz"))
        );

        write(
            &dir.path().join("ferestre-dlssnr-0.1.0-x86_64.tar.gz"),
            b"newer",
        );
        let error = local(dir.path(), Arch::X86_64).unwrap_err();
        assert!(error.to_string().contains("more than one"), "{error}");

        let empty = tempfile::tempdir().unwrap();
        let error = local(empty.path(), Arch::I686).unwrap_err();
        assert!(error.to_string().contains("build-dlssnr.sh"), "{error}");

        // A name that would leave the packages folder is not a version.
        let odd = tempfile::tempdir().unwrap();
        write(&odd.path().join("ferestre-dlssnr-..-x86_64.tar.gz"), b"x");
        assert!(local(odd.path(), Arch::X86_64).is_err());
    }

    #[test]
    fn the_local_override_comes_before_the_pin() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("ferestre-dlssnr-0.0.9-x86_64.tar.gz"),
            b"tarball",
        );
        let home = tempfile::tempdir().unwrap();
        let env = |vars: Vec<(&str, String)>| {
            let mut all = vec![("HOME", home.path().display().to_string())];
            all.extend(vars);
            Paths::resolve(&crate::paths::Env::from_map(all)).unwrap()
        };
        let paths = env(vec![(LOCAL_ENV, dir.path().display().to_string())]);
        let pkg = current(&paths, Arch::X86_64).unwrap();
        assert!(matches!(pkg.origin, Origin::File(_)));

        let paths = env(vec![]);
        match (current(&paths, Arch::X86_64), pinned(Arch::X86_64)) {
            (Ok(pkg), Some(pin)) => assert_eq!(pkg, pin),
            (Err(e), None) => {
                let text = e.to_string();
                assert!(text.contains(LOCAL_ENV) && text.contains("scripts/build-dlssnr.sh"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_pin_is_empty_or_a_ferestre_release_asset() {
        for arch in Arch::ALL {
            let (url, sha256) = pinned_asset(arch);
            if url.is_empty() && sha256.is_empty() {
                assert_eq!(pinned(arch), None);
                continue;
            }
            assert!(url.starts_with(RELEASE_PREFIX), "{url}");
            assert!(
                url.ends_with(&format!(
                    "dlssnr-{PINNED_VERSION}/{}",
                    asset_name(PINNED_VERSION, arch)
                )),
                "{url}"
            );
            assert_eq!(sha256.len(), 64, "{sha256}");
            assert!(
                sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{sha256}"
            );
            assert!(pinned(arch).is_some());
        }
    }

    fn fake_package() -> tempfile::TempDir {
        let pkg = tempfile::tempdir().unwrap();
        let p = pkg.path();
        write(&p.join("install.sh"), b"#!/bin/bash\n");
        write(&p.join("dlssnr-amd/shaders/a.spv"), b"x");
        write(&p.join("reshade/dxgi.dll"), b"reshade");
        write(&p.join("reshade/ReShade.ini"), b"ini");
        write(&p.join("reshade/reshade-shaders/Shaders/x.fx"), b"fx");
        write(&p.join("vulkan/vulkan-1.dll"), b"loader");
        write(&p.join("vulkan/winevulkan.dll"), b"wv");
        write(&p.join("vulkan/ReShadePreset-d3d9.ini"), b"d3d9");
        write(&p.join("vulkan/vk-override/x.json"), b"{}");
        write(&p.join("optiscaler/release/OptiScaler.dll"), b"opti");
        write(&p.join("optiscaler/release/OptiScaler.ini"), b"ini");
        write(&p.join("optiscaler/release/OptiScaler/x"), b"x");
        write(&p.join("optiscaler/vulkan-1.dll"), b"forwarder");
        pkg
    }

    fn by_name(files: &[RouteFile]) -> BTreeSet<&str> {
        files.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn route_files_follow_the_package_contents() {
        let pkg = fake_package();
        let p = pkg.path();

        let feeder = route_files(p, Route::ReshadeFeeder).unwrap();
        let names = by_name(&feeder);
        for name in [
            "dlssnr-amd-install.txt",
            "dlssnr-amd",
            "dlssnr-amd.ini",
            "dlssnr-amd.log",
            "ReShade.log",
            "dxgi.dll",
            "ReShade.ini",
            "reshade-shaders",
        ] {
            assert!(names.contains(name), "{name} in {names:?}");
        }
        let shaders = feeder.iter().find(|f| f.name == "reshade-shaders").unwrap();
        assert!(shaders.dir);
        assert_eq!(shaders.source, Some(p.join("reshade/reshade-shaders")));
        assert!(!names.contains("vulkan-1.dll"));

        let vulkan = route_files(p, Route::ReshadeFeederVulkan).unwrap();
        let names = by_name(&vulkan);
        for name in [
            "vulkan-1.dll",
            "winevulkan.dll",
            "vk-override",
            "ReShade.ini",
        ] {
            assert!(names.contains(name), "{name} in {names:?}");
        }
        assert!(!names.contains("dxgi.dll"));
        assert!(!names.contains("ReShadePreset-d3d9.ini"));
        assert!(!names.contains("ReShadePreset.ini"));

        let dx9 = route_files(p, Route::ReshadeFeederDx9).unwrap();
        let preset = dx9.iter().find(|f| f.name == "ReShadePreset.ini").unwrap();
        assert_eq!(preset.source, Some(p.join("vulkan/ReShadePreset-d3d9.ini")));

        let opti = route_files(p, Route::Optiscaler).unwrap();
        let names = by_name(&opti);
        for name in [
            "dlssnr-amd-install.txt",
            "dlssnr-amd",
            "dlssnr-amd.ini",
            "dlssnr-amd.log",
            "dxgi.dll",
            "OptiScaler.ini",
            "OptiScaler",
            "OptiScaler.log",
            "vulkan-1.dll",
        ] {
            assert!(names.contains(name), "{name} in {names:?}");
        }
        assert!(!names.contains("ReShade.ini"));
        let dxgi = opti.iter().find(|f| f.name == "dxgi.dll").unwrap();
        assert_eq!(
            dxgi.source,
            Some(p.join("optiscaler/release/OptiScaler.dll"))
        );
        let absent: Vec<&str> = opti
            .iter()
            .filter(|f| f.only_if_absent)
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(absent, vec!["vulkan-1.dll"]);
        let vulkan_dll = opti.iter().find(|f| f.name == "vulkan-1.dll").unwrap();
        assert_eq!(vulkan_dll.source, Some(p.join("optiscaler/vulkan-1.dll")));
        for files in [&feeder, &vulkan, &dx9] {
            assert!(files.iter().all(|f| !f.only_if_absent));
        }

        // The 32-bit package has no OptiScaler route.
        std::fs::remove_dir_all(p.join("optiscaler")).unwrap();
        let error = route_files(p, Route::Optiscaler).unwrap_err();
        assert!(error.to_string().contains("64-bit"), "{error}");
    }

    #[test]
    fn injectors_name_what_each_route_loads_through() {
        let opti = injectors(Route::Optiscaler);
        assert_eq!(opti.required, &["dxgi.dll"]);
        assert_eq!(opti.when_listed, &["vulkan-1.dll"]);
        assert_eq!(injectors(Route::ReshadeFeeder).required, &["dxgi.dll"]);
        assert!(injectors(Route::ReshadeFeeder).when_listed.is_empty());
        for route in [Route::ReshadeFeederVulkan, Route::ReshadeFeederDx9] {
            let i = injectors(route);
            assert_eq!(i.required, &["vulkan-1.dll", "winevulkan.dll"]);
            assert!(i.when_listed.is_empty());
        }
    }

    #[test]
    fn a_package_expecting_other_models_is_detected() {
        let pkg = tempfile::tempdir().unwrap();
        let expect = Expect::pinned();
        let script = |model: &str, small: &str| {
            format!("#!/bin/bash\nset -e\nmodel_sha256={model}\nsmall_sha256={small}\n")
        };
        write(
            &pkg.path().join("install.sh"),
            script(&expect.aardwold.sha256, &expect.small.sha256).as_bytes(),
        );
        assert!(expects_models(pkg.path(), &expect));
        write(
            &pkg.path().join("install.sh"),
            script(&"1".repeat(64), &expect.small.sha256).as_bytes(),
        );
        assert!(!expects_models(pkg.path(), &expect));
        write(
            &pkg.path().join("install.sh"),
            script(&expect.aardwold.sha256, &"2".repeat(64)).as_bytes(),
        );
        assert!(!expects_models(pkg.path(), &expect));
        std::fs::remove_file(pkg.path().join("install.sh")).unwrap();
        assert!(!expects_models(pkg.path(), &expect));
    }
}
