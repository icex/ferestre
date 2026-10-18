//! Read an Appx or Msix package manifest without extracting the archive.
//!
//! A package being readable says nothing about it being launchable. This module
//! deliberately reports that distinction instead of trying to launch an archive
//! as soon as it can parse one.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
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
    let too_big = || {
        anyhow!(
            "{MANIFEST} exceeds the {} MiB inspection limit",
            MAX_MANIFEST_BYTES / 1024 / 1024
        )
    };
    if manifest.size() > MAX_MANIFEST_BYTES {
        return Err(too_big());
    }
    // The size above is what the archive claims, and a crafted one can claim
    // a few bytes for a member that inflates to gigabytes. The cap is applied
    // to what actually comes out: one byte past it is enough to know.
    let mut bytes = Vec::with_capacity(manifest.size() as usize);
    (&mut manifest)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("AppxManifest.xml could not be read")?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(too_big());
    }
    let text = String::from_utf8(bytes).context("AppxManifest.xml is not UTF-8 XML")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A deflated manifest of `real` bytes whose headers claim it is `claimed`
    /// bytes long, as a hostile package would.
    fn lying_archive(real: usize, claimed: u32) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("temporary archive");
        let mut zip = zip::ZipWriter::new(File::create(file.path()).expect("create"));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(MANIFEST, options).expect("manifest entry");
        let mut body = b"<Package><Identity Name=\"X\" Version=\"1.0.0.0\"/>".to_vec();
        body.resize(real, b' ');
        zip.write_all(&body).expect("manifest data");
        zip.finish().expect("finish archive");

        let mut bytes = std::fs::read(file.path()).unwrap();
        let patch = |bytes: &mut Vec<u8>, signature: [u8; 4], offset: usize| {
            let at = bytes
                .windows(4)
                .position(|w| w == signature)
                .expect("header present");
            bytes[at + offset..at + offset + 4].copy_from_slice(&claimed.to_le_bytes());
        };
        // Uncompressed size in the local header and in the central directory.
        patch(&mut bytes, [0x50, 0x4b, 0x03, 0x04], 22);
        patch(&mut bytes, [0x50, 0x4b, 0x01, 0x02], 24);
        std::fs::write(file.path(), bytes).unwrap();
        file
    }

    #[test]
    fn the_cap_holds_against_a_manifest_that_lies_about_its_size() {
        let archive = lying_archive(MAX_MANIFEST_BYTES as usize + 4096, 64);
        let err = inspect_archive(archive.path())
            .expect_err("an oversized manifest is refused")
            .to_string();
        assert!(err.contains("inspection limit"), "{err}");
    }

    #[test]
    fn an_honest_manifest_under_the_cap_is_read() {
        let archive = lying_archive(4096, 4096);
        let package = inspect_archive(archive.path()).expect("inspect");
        assert_eq!(package.identity.as_deref(), Some("X"));
    }
}
