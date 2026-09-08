use std::fs::File;
use std::io::Write;
use std::path::Path;

use ferestre_core::appx::{inspect_archive, PackageKind};
use zip::write::SimpleFileOptions;

fn archive(manifest: Option<&str>) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("temporary archive");
    let mut zip = zip::ZipWriter::new(File::create(file.path()).expect("create"));
    if let Some(manifest) = manifest {
        zip.start_file("AppxManifest.xml", SimpleFileOptions::default())
            .expect("manifest entry");
        zip.write_all(manifest.as_bytes()).expect("manifest data");
    }
    zip.finish().expect("finish archive");
    file
}

fn inspect(manifest: Option<&str>) -> anyhow::Result<ferestre_core::appx::Package> {
    let archive = archive(manifest);
    inspect_archive(Path::new(archive.path()))
}

#[test]
fn full_trust_manifest_is_a_desktop_bridge() {
    let package = inspect(Some(
        r#"<Package><Identity Name="Example.Game" Version="1.2.3.4"/>
        <Applications><Application Id="Game" Executable="Game.exe"
        EntryPoint="Windows.FullTrustApplication"/></Applications></Package>"#,
    ))
    .expect("inspect");

    assert_eq!(package.identity.as_deref(), Some("Example.Game"));
    assert_eq!(package.version.as_deref(), Some("1.2.3.4"));
    assert_eq!(package.executable.as_deref(), Some("Game.exe"));
    assert_eq!(package.kind, PackageKind::DesktopBridge);
}

#[test]
fn non_full_trust_manifest_is_uwp() {
    let package = inspect(Some(
        r#"<Package><Applications><Application Id="App" Executable="App.exe"
        EntryPoint="Contoso.App"/></Applications></Package>"#,
    ))
    .expect("inspect");

    assert_eq!(package.kind, PackageKind::Uwp);
}

#[test]
fn archive_without_manifest_is_rejected() {
    let err = inspect(None).expect_err("manifest required");
    assert!(err.to_string().contains("AppxManifest.xml"));
}
