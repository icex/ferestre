//! Whether this machine can run DLSSNR-AMD: an RDNA4 GPU and Mesa 26.2 or
//! newer. Only ever warnings; nothing here blocks enabling.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const AMD: u16 = 0x1002;
/// PCI device ids of RDNA4 (Navi 48 and Navi 44) cards; newer ones are
/// recognised by their graphics IP version instead.
pub const RDNA4_DEVICES: &[u16] = &[0x7550, 0x7551, 0x7590];
pub const MIN_MESA: (u32, u32) = (26, 2);
const VULKANINFO_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpu {
    pub vendor: u16,
    pub device: u16,
    /// The graphics IP's major version (12 is RDNA4) when the kernel reports
    /// it.
    pub gfx_major: Option<u32>,
}

impl Gpu {
    fn is_rdna4(&self) -> bool {
        self.vendor == AMD && (RDNA4_DEVICES.contains(&self.device) || self.gfx_major == Some(12))
    }
}

fn hex(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    u16::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).ok()
}

/// The GPUs under a `/sys/class/drm`-shaped directory (`card<N>`, not its
/// connectors).
pub fn gpus(drm: &Path) -> Vec<Gpu> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut cards: Vec<_> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("card"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path().join("device"))
        .collect();
    cards.sort();
    cards
        .into_iter()
        .filter_map(|device| {
            Some(Gpu {
                vendor: hex(&device.join("vendor"))?,
                device: hex(&device.join("device"))?,
                gfx_major: std::fs::read_to_string(device.join("ip_discovery/die/0/GC/0/major"))
                    .ok()
                    .and_then(|t| t.trim().parse().ok()),
            })
        })
        .collect()
}

/// The first `Mesa <major>.<minor>` on a `driverInfo` line of
/// `vulkaninfo --summary`.
pub fn mesa_version(vulkan_summary: &str) -> Option<(u32, u32)> {
    vulkan_summary
        .lines()
        .filter(|line| line.contains("driverInfo"))
        .find_map(|line| {
            let rest = &line[line.find("Mesa ")? + "Mesa ".len()..];
            let mut parts = rest.split(|c: char| !c.is_ascii_digit());
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            Some((major, minor))
        })
}

/// What to tell the person. `checked_mesa` is whether `vulkaninfo` could be
/// run at all.
pub fn warnings(gpus: &[Gpu], mesa: Option<(u32, u32)>, checked_mesa: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    if !gpus.iter().any(Gpu::is_rdna4) {
        warnings.push(
            "No AMD RX 9000 (RDNA4) GPU was found; DLSSNR-AMD runs only on RDNA4 and stays off \
             in game without it."
                .to_string(),
        );
    }
    let (min_major, min_minor) = MIN_MESA;
    match (checked_mesa, mesa) {
        (true, Some(version)) if version < MIN_MESA => warnings.push(format!(
            "Mesa {}.{} is older than {min_major}.{min_minor}, which DLSSNR-AMD needs.",
            version.0, version.1
        )),
        (true, Some(_)) => {}
        (true, None) => warnings.push(format!(
            "Could not read a Mesa version from vulkaninfo; DLSSNR-AMD needs Mesa \
             {min_major}.{min_minor} or newer."
        )),
        (false, _) => warnings.push(format!(
            "Could not check the Mesa version (vulkaninfo is not installed); DLSSNR-AMD needs \
             Mesa {min_major}.{min_minor} or newer."
        )),
    }
    warnings
}

/// `vulkaninfo --summary`'s output, or `None` when it is not on `PATH`.
/// Empty when it fails or runs past its time limit.
fn vulkan_summary() -> Option<String> {
    let mut child = Command::new("vulkaninfo")
        .arg("--summary")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + VULKANINFO_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(String::new());
            }
        }
    }
    Some(reader.join().unwrap_or_default())
}

/// The warnings for this machine.
pub fn check() -> Vec<String> {
    let gpus = gpus(Path::new("/sys/class/drm"));
    let summary = vulkan_summary();
    warnings(
        &gpus,
        summary.as_deref().and_then(mesa_version),
        summary.is_some(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(drm: &Path, name: &str, vendor: &str, device: &str, gfx: Option<&str>) {
        let dir = drm.join(name).join("device");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(dir.join("device"), format!("{device}\n")).unwrap();
        if let Some(gfx) = gfx {
            let gc = dir.join("ip_discovery/die/0/GC/0");
            std::fs::create_dir_all(&gc).unwrap();
            std::fs::write(gc.join("major"), format!("{gfx}\n")).unwrap();
        }
    }

    const SUMMARY: &str = "Devices:\n========\nGPU0:\n\
        \tdriverName         = radv\n\
        \tdriverInfo         = Mesa 26.2.0-arch1.1 (LLVM 23.1.5)\n";

    #[test]
    fn an_rx_9070_xt_with_mesa_26_2_has_no_warnings() {
        let drm = tempfile::tempdir().unwrap();
        card(drm.path(), "card1", "0x1002", "0x7550", Some("12"));
        // A connector is not a GPU.
        std::fs::create_dir_all(drm.path().join("card1-DP-1")).unwrap();
        let gpus = gpus(drm.path());
        assert_eq!(
            gpus,
            vec![Gpu {
                vendor: 0x1002,
                device: 0x7550,
                gfx_major: Some(12)
            }]
        );
        assert_eq!(mesa_version(SUMMARY), Some((26, 2)));
        assert_eq!(
            warnings(&gpus, mesa_version(SUMMARY), true),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_rdna3_card_and_old_mesa_both_warn() {
        let drm = tempfile::tempdir().unwrap();
        card(drm.path(), "card0", "0x1002", "0x744c", Some("11"));
        let summary = "\tdriverInfo         = Mesa 25.3.4\n";
        let warnings = warnings(&gpus(drm.path()), mesa_version(summary), true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("RDNA4"), "{warnings:?}");
        assert_eq!(
            warnings[1],
            "Mesa 25.3 is older than 26.2, which DLSSNR-AMD needs."
        );
    }

    #[test]
    fn a_gfx12_device_id_not_in_the_list_still_counts() {
        let drm = tempfile::tempdir().unwrap();
        card(drm.path(), "card0", "0x1002", "0x75ff", Some("12"));
        let warnings = warnings(&gpus(drm.path()), Some((27, 0)), true);
        assert_eq!(warnings, Vec::<String>::new());
    }

    #[test]
    fn an_unchecked_mesa_and_another_vendor_warn() {
        let drm = tempfile::tempdir().unwrap();
        card(drm.path(), "card0", "0x10de", "0x7550", None);
        let warnings = warnings(&gpus(drm.path()), None, false);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[1].contains("vulkaninfo is not installed"),
            "{warnings:?}"
        );
        assert_eq!(mesa_version("deviceName = AMD\n"), None);
    }
}
