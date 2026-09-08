//! Read an Appx or Msix package manifest without extracting the archive.
//!
//! A package being readable says nothing about it being launchable. This module
//! deliberately reports that distinction instead of trying to launch an archive
//! as soon as it can parse one.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

/// An upper bound for the one XML file this module reads.
///
/// Real manifests are small. The cap makes a hostile archive fail before an
/// otherwise harmless inspection turns into an unbounded allocation.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MANIFEST: &str = "AppxManifest.xml";

/// The execution model declared by the package itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    /// An ordinary Win32 program delivered in Store packaging.
    DesktopBridge,
    /// An application that needs UWP activation and app-model services.
    Uwp,
    /// No executable application declaration was available to classify.
    Unknown,
}

/// The small, inspectable part of an Appx/Msix package manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Package {
    pub identity: Option<String>,
    pub version: Option<String>,
    pub executable: Option<String>,
    pub entry_point: Option<String>,
    pub kind: PackageKind,
}

/// Inspect the archive's manifest without writing any archive member to disk.
pub fn inspect_archive(path: &Path) -> Result<Package> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not an Appx/Msix zip archive", path.display()))?;
    let mut manifest = archive
        .by_name(MANIFEST)
        .map_err(|_| anyhow!("{MANIFEST} is missing from {}", path.display()))?;
    if manifest.size() > MAX_MANIFEST_BYTES {
        bail!(
            "{MANIFEST} exceeds the {} MiB inspection limit",
            MAX_MANIFEST_BYTES / 1024 / 1024
        );
    }
    let mut text = String::with_capacity(manifest.size() as usize);
    manifest
        .read_to_string(&mut text)
        .context("AppxManifest.xml is not UTF-8 XML")?;
    Ok(parse_manifest(&text))
}

fn parse_manifest(text: &str) -> Package {
    let identity = element(text, "Identity");
    let application = element(text, "Application");
    let entry_point = application.and_then(|tag| attribute(tag, "EntryPoint"));
    let executable = application.and_then(|tag| attribute(tag, "Executable"));
    let kind = match entry_point.as_deref() {
        Some("Windows.FullTrustApplication") => PackageKind::DesktopBridge,
        Some(_) => PackageKind::Uwp,
        None if executable.is_some() => PackageKind::Uwp,
        None => PackageKind::Unknown,
    };
    Package {
        identity: identity.and_then(|tag| attribute(tag, "Name")),
        version: identity.and_then(|tag| attribute(tag, "Version")),
        executable,
        entry_point,
        kind,
    }
}

fn element<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("<{name} "))?;
    let end = text[start..].find('>')? + start;
    Some(&text[start..=end])
}

fn attribute(element: &str, name: &str) -> Option<String> {
    let at = element.find(&format!("{name}="))?;
    let rest = &element[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    if quote != '\"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let close = value.find(quote)?;
    let value = value[..close].trim();
    (!value.is_empty()).then(|| value.to_string())
}
