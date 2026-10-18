//! Diagnose the bundled GStreamer plugin with the same library search path as
//! Proton. A current system FFmpeg cannot satisfy the FFmpeg 4 ABI it needs,
//! but a Steam install ships those sonames and Proton puts them first.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEPENDENCY_HINT: &str = "Fedora (with RPM Fusion enabled): sudo dnf install compat-ffmpeg4. Arch/CachyOS: install ffmpeg4.4. See docs/DEPENDENCIES.md";

/// Missing dependencies of the 64-bit plugin. An unavailable diagnostic is an
/// error, never evidence that the plugin works. This does not prove decoding.
///
/// `steam` is the install the launch hands Proton as
/// `STEAM_COMPAT_CLIENT_INSTALL_PATH`, when there is one.
pub fn missing_libav_dependencies(runtime: &Path, steam: Option<&Path>) -> Result<Vec<String>> {
    let plugin = runtime.join("files/lib/x86_64-linux-gnu/gstreamer-1.0/libgstlibav.so");
    if !plugin.is_file() {
        bail!("the runtime has no 64-bit GStreamer libav plugin");
    }
    let output = Command::new("ldd")
        .arg(&plugin)
        .env("LC_ALL", "C")
        .env(
            "LD_LIBRARY_PATH",
            std::env::join_paths(proton_library_path(runtime, steam))?,
        )
        .output()
        .context("checking media dependencies with ldd")?;
    if !output.status.success() {
        bail!("ldd could not inspect the runtime's GStreamer libav plugin");
    }
    Ok(missing_from_ldd(&String::from_utf8_lossy(&output.stdout)))
}

/// The directories Proton puts on LD_LIBRARY_PATH, in its order. init_wine
/// prepends the runtime's own library directories, then setup_prefix prepends
/// Steam's video directories in front of them, so Steam's FFmpeg 4 wins.
///
/// Whatever LD_LIBRARY_PATH this process inherited is left out: the launch
/// removes it before starting Proton, so it cannot help the game.
fn proton_library_path(runtime: &Path, steam: Option<&Path>) -> Vec<PathBuf> {
    let mut search = Vec::new();
    if let Some(steam) = steam {
        search.extend(
            [
                "ubuntu12_64/video",
                "ubuntu12_32/video",
                "steamrtarm64/video",
            ]
            .iter()
            .map(|dir| steam.join(dir)),
        );
    }
    let lib = runtime.join("files/lib");
    search.extend(
        ["x86_64-linux-gnu", "aarch64-linux-gnu", "i386-linux-gnu"]
            .iter()
            .map(|dir| lib.join(dir)),
    );
    search
}

fn missing_from_ldd(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let (name, resolution) = line.trim().split_once(" => ")?;
            (resolution.trim() == "not found").then(|| name.to_string())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_sonames_are_reported_once_without_paths_or_addresses() {
        assert_eq!(
            missing_from_ldd("\tlibavcodec.so.58 => not found\nlibavutil.so.56 => /usr/lib/libavutil.so.56 (0x123)\nlibavcodec.so.58 => not found\nlibavfilter.so.7 => not found\n"),
            vec!["libavcodec.so.58", "libavfilter.so.7"]
        );
        assert!(missing_from_ldd("libc.so.6 => /usr/lib/libc.so.6 (0x123)").is_empty());
    }

    #[test]
    fn steam_video_libraries_come_before_the_runtime_like_proton() {
        let runtime = Path::new("/rt");
        assert_eq!(
            proton_library_path(runtime, Some(Path::new("/steam"))),
            [
                "/steam/ubuntu12_64/video",
                "/steam/ubuntu12_32/video",
                "/steam/steamrtarm64/video",
                "/rt/files/lib/x86_64-linux-gnu",
                "/rt/files/lib/aarch64-linux-gnu",
                "/rt/files/lib/i386-linux-gnu",
            ]
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
        );
        assert_eq!(proton_library_path(runtime, None).len(), 3);
    }
}
