//! Diagnose the bundled GStreamer plugin with the same library search path as
//! Proton. A current system FFmpeg cannot satisfy the FFmpeg 4 ABI it needs.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

pub const DEPENDENCY_HINT: &str = "Fedora (with RPM Fusion enabled): sudo dnf install compat-ffmpeg4. Arch/CachyOS: install ffmpeg4.4. See docs/DEPENDENCIES.md";

/// Missing dependencies of the 64-bit plugin. An unavailable diagnostic is an
/// error, never evidence that the plugin works. This does not prove decoding.
pub fn missing_libav_dependencies(runtime: &Path) -> Result<Vec<String>> {
    let lib = runtime.join("files/lib");
    let plugin = lib.join("x86_64-linux-gnu/gstreamer-1.0/libgstlibav.so");
    if !plugin.is_file() {
        bail!("the runtime has no 64-bit GStreamer libav plugin");
    }
    let mut search = vec![
        lib.join("x86_64-linux-gnu"),
        lib.join("aarch64-linux-gnu"),
        lib.join("i386-linux-gnu"),
    ];
    if let Some(inherited) = std::env::var_os("LD_LIBRARY_PATH") {
        search.extend(std::env::split_paths(&inherited));
    }
    let output = Command::new("ldd")
        .arg(&plugin)
        .env("LC_ALL", "C")
        .env("LD_LIBRARY_PATH", std::env::join_paths(search)?)
        .output()
        .context("checking media dependencies with ldd")?;
    if !output.status.success() {
        bail!("ldd could not inspect the runtime's GStreamer libav plugin");
    }
    Ok(missing_from_ldd(&String::from_utf8_lossy(&output.stdout)))
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
}
